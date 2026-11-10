//! `git clone` for the Add project dialog's "Clone from GitHub" tab.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

const CLONE_TIMEOUT: Duration = Duration::from_secs(120);

/// The folder a repository clones into: its last path segment without `.git`.
pub fn repo_name(repo: &str) -> Option<String> {
    let repo = repo.trim().trim_end_matches('/');
    if repo.is_empty() || repo.starts_with('-') {
        return None;
    }
    let path = ["https://", "http://", "ssh://", "git://"]
        .iter()
        .find_map(|scheme| repo.strip_prefix(scheme))
        .unwrap_or(repo);
    let path = path.rsplit_once('@').map_or(path, |(_, rest)| rest);
    // `git@host:owner/repo` separates owner and repository with a colon; a URL needs both segments.
    let path = path.replace(':', "/");
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let owner = segments.next()?;
    let last = segments.next_back().or(Some(owner))?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    if owner == last || name.is_empty() || name == "." || name == ".." || name.starts_with('.') {
        return None;
    }
    Some(name.to_string())
}

/// `~` and `~/...` are the daemon user's home, as the dialog's default shows.
pub fn expand_location(location: &str) -> PathBuf {
    let location = location.trim();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if location == "~" {
        return home.unwrap_or_else(|| PathBuf::from(location));
    }
    match (location.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(location),
    }
}

/// Clone into `target`, which must not exist yet, and answer its path.
pub async fn clone_into(repo: &str, target: &Path) -> Result<PathBuf, String> {
    if repo_name(repo).is_none() {
        return Err(format!("Not a repository: {repo}"));
    }
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let clone = tokio::process::Command::new("git")
        .arg("clone")
        .arg("--")
        .arg(repo)
        .arg(target)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(CLONE_TIMEOUT, clone)
        .await
        .map_err(|_| format!("git clone timed out after {}s", CLONE_TIMEOUT.as_secs()))?
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let line = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("git clone failed");
        return Err(format!("git clone failed: {}", line.trim()));
    }
    Ok(target.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn bare_repo(dir: &Path) -> PathBuf {
        let path = dir.join("origin.git");
        let status = std::process::Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        path
    }

    #[test]
    fn names_the_folder_from_the_repository() {
        assert_eq!(
            repo_name("https://github.com/yannelli/emberloop-rs.git").as_deref(),
            Some("emberloop-rs")
        );
        assert_eq!(
            repo_name("git@github.com:yannelli/emberloop-rs.git").as_deref(),
            Some("emberloop-rs")
        );
        assert_eq!(
            repo_name("https://github.com/yannelli/emberloop-rs/").as_deref(),
            Some("emberloop-rs")
        );
        assert_eq!(repo_name("-uhttps://example.com/x").as_deref(), None);
        assert_eq!(repo_name("https://github.com/").as_deref(), None);
        assert_eq!(repo_name("https://example.com/..").as_deref(), None);
    }

    #[tokio::test]
    async fn clones_into_a_new_folder_and_refuses_an_existing_one() {
        let dir = tempdir().unwrap();
        let repo = bare_repo(dir.path());
        let target = dir.path().join("projects").join("origin");

        clone_into(repo.to_str().unwrap(), &target).await.unwrap();
        assert!(target.join(".git").is_dir(), "clone has a git directory");

        let again = clone_into(repo.to_str().unwrap(), &target).await;
        assert!(again.unwrap_err().contains("already exists"));
    }
}
