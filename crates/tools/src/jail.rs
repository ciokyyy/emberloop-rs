use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{ToolError, ToolResult};

/// Resolves tool paths. `resolve` confines `grep` and `find` to the project root; `resolve_any`
/// and `resolve_writable` reach any path the account can open, because a refused edit only pushes
/// the model into the shell, which is no sandbox anyway.
#[derive(Clone, Debug)]
pub struct ProjectJail {
    root: PathBuf,
    /// Set by `with_extra_root`; `None` keeps `grep` and `find` inside the project root.
    global_skills: Option<PathBuf>,
}

impl ProjectJail {
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = fs::canonicalize(root)?;
        Ok(Self {
            root,
            global_skills: None,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Also lets paths reach the global skills root and each skill linked directly under it.
    pub fn with_extra_root(mut self, global: impl AsRef<Path>) -> Self {
        self.global_skills = Some(global.as_ref().to_path_buf());
        self
    }

    pub fn resolve(&self, raw: &str) -> ToolResult<PathBuf> {
        let resolved = self.resolve_any(raw)?;
        if !self.allows(&resolved) {
            return Err(ToolError::msg(format!(
                "{} is outside the project root {}",
                resolved.display(),
                self.root.display()
            )));
        }
        Ok(resolved)
    }

    /// `resolve` plus `session`, a goldfish's own Session directory: the allowlist of its `read`.
    pub fn resolve_goldfish(&self, raw: &str, session: &Path) -> ToolResult<PathBuf> {
        let resolved = self.resolve_any(raw)?;
        let own = fs::canonicalize(session).ok();
        if self.allows(&resolved) || own.is_some_and(|own| resolved.starts_with(own)) {
            return Ok(resolved);
        }
        Err(ToolError::msg(format!(
            "{} is outside the project root, the skill roots and this Session",
            resolved.display()
        )))
    }

    /// Any existing path: `~` expands from `HOME`, a relative path joins the project root.
    pub fn resolve_any(&self, raw: &str) -> ToolResult<PathBuf> {
        let candidate = self.candidate(raw)?;
        fs::canonicalize(&candidate)
            .map_err(|e| ToolError::msg(format!("{}: {e}", candidate.display())))
    }

    /// A path that may not exist yet: the deepest existing ancestor is canonicalized (so a
    /// symlink resolves) and the missing tail is appended.
    pub fn resolve_writable(&self, raw: &str) -> ToolResult<PathBuf> {
        let mut resolved = normalize_lexical(&self.candidate(raw)?);
        let mut tail = Vec::new();
        while !resolved.exists() {
            let Some(name) = resolved.file_name() else {
                break;
            };
            tail.push(name.to_os_string());
            let Some(parent) = resolved.parent() else {
                break;
            };
            resolved = parent.to_path_buf();
        }
        let mut resolved = fs::canonicalize(&resolved)
            .map_err(|e| ToolError::msg(format!("{}: {e}", resolved.display())))?;
        for part in tail.into_iter().rev() {
            resolved.push(part);
        }
        Ok(resolved)
    }

    fn candidate(&self, raw: &str) -> ToolResult<PathBuf> {
        let expanded = expand_home(raw)?;
        if expanded.is_empty() {
            return Err(ToolError::msg("path is required"));
        }
        let given = Path::new(&expanded);
        Ok(if given.is_absolute() {
            given.to_path_buf()
        } else {
            self.root.join(given)
        })
    }

    fn allows(&self, resolved: &Path) -> bool {
        resolved.starts_with(&self.root)
            || self
                .skill_roots()
                .iter()
                .any(|root| resolved.starts_with(root))
    }

    /// Global root plus each direct child's target, listed per call so new links count at once.
    fn skill_roots(&self) -> Vec<PathBuf> {
        let global = self
            .global_skills
            .as_ref()
            .and_then(|path| fs::canonicalize(path).ok());
        let Some(global) = global else {
            return Vec::new();
        };
        let mut roots: Vec<PathBuf> = fs::read_dir(&global)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| fs::canonicalize(entry.path()).ok())
            .filter(|target| target.is_dir())
            .collect();
        roots.push(global);
        roots
    }
}

/// `~` and `~/x` expand from `HOME`; `~user` stays a literal file name.
fn expand_home(raw: &str) -> ToolResult<String> {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix('~') else {
        return Ok(trimmed.to_string());
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return Ok(trimmed.to_string());
    }
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return Err(ToolError::msg(format!(
            "cannot expand {trimmed}: HOME is not set"
        )));
    };
    Ok(format!("{}{rest}", PathBuf::from(home).to_string_lossy()))
}

fn normalize_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    use super::ProjectJail;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn text(path: &Path) -> String {
        path.display().to_string()
    }

    #[test]
    fn global_skills_resolve_with_symlinked_skill_and_refuse_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let project = base.join("project");
        let global = base.join("home/.agents/skills");
        write(&project.join("a.txt"), "project");
        write(&global.join("foo/SKILL.md"), "foo");
        write(&base.join("elsewhere/bar/SKILL.md"), "bar");
        write(&base.join("home/.agents/skills-old/secret.txt"), "sibling");
        write(&base.join("home/.agents/secret.txt"), "parent");
        write(&base.join("unrelated/secret.txt"), "unrelated");
        let jail = ProjectJail::new(&project).unwrap().with_extra_root(&global);
        // Skill links are listed per call, so a link made after the jail was built still resolves.
        symlink(base.join("elsewhere/bar"), global.join("bar")).unwrap();
        symlink(base.join("unrelated"), project.join("link")).unwrap();
        symlink(base.join("unrelated"), global.join("foo/escape")).unwrap();

        let readable: [PathBuf; 3] = [
            project.join("a.txt"),
            global.join("foo/SKILL.md"),
            global.join("bar/SKILL.md"),
        ];
        for path in readable {
            assert!(jail.resolve(&text(&path)).is_ok(), "{}", path.display());
        }
        let refused: [PathBuf; 5] = [
            base.join("home/.agents/skills-old/secret.txt"),
            global.join("../secret.txt"),
            project.join("link/secret.txt"),
            global.join("foo/escape/secret.txt"),
            base.join("unrelated/secret.txt"),
        ];
        for path in refused {
            assert!(jail.resolve(&text(&path)).is_err(), "{}", path.display());
        }
    }

    #[test]
    fn global_skills_stay_refused_without_the_extra_root() {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let project = base.join("project");
        let global = base.join("home/.agents/skills");
        write(&project.join("a.txt"), "project");
        write(&global.join("foo/SKILL.md"), "foo");
        let jail = ProjectJail::new(&project).unwrap();
        assert!(jail.resolve(&text(&project.join("a.txt"))).is_ok());
        assert!(jail.resolve(&text(&global.join("foo/SKILL.md"))).is_err());
    }

    /// `HOME` override for the `~` tests; restored when the guard drops.
    struct HomeGuard(Option<std::ffi::OsString>);

    impl HomeGuard {
        fn set(path: &Path) -> Self {
            let previous = std::env::var_os("HOME");
            std::env::set_var("HOME", path);
            Self(previous)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var("HOME", previous),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[test]
    fn edit_and_write_reach_past_the_root_while_the_jail_still_confines_grep() {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let project = base.join("project");
        let outside = base.join("outside/notes.txt");
        write(&project.join("a.txt"), "project");
        write(&outside, "outside");
        let jail = ProjectJail::new(&project).unwrap();
        assert!(jail.resolve(&text(&outside)).is_err());
        assert_eq!(jail.resolve_any(&text(&outside)).unwrap(), outside);
        assert_eq!(jail.resolve_any("a.txt").unwrap(), project.join("a.txt"));
        assert_eq!(
            jail.resolve_writable("../outside/new/notes.txt").unwrap(),
            base.join("outside/new/notes.txt")
        );
        // A symlink still resolves to its target, outside the root included.
        symlink(&outside, project.join("link.txt")).unwrap();
        assert_eq!(
            jail.resolve_any(&text(&project.join("link.txt"))).unwrap(),
            outside
        );
    }

    #[test]
    fn tilde_expands_from_home() {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let project = base.join("project");
        write(&project.join("a.txt"), "project");
        write(&base.join("home/notes.txt"), "notes");
        let jail = ProjectJail::new(&project).unwrap();
        let _home = HomeGuard::set(&base.join("home"));
        assert_eq!(
            jail.resolve_any("~/notes.txt").unwrap(),
            base.join("home/notes.txt")
        );
        assert_eq!(
            jail.resolve_writable("~/new/dir/x.txt").unwrap(),
            base.join("home/new/dir/x.txt")
        );
        // `~user` is a literal file name, not a home lookup.
        assert!(jail.resolve_any("~nobody/notes.txt").is_err());
    }

    #[test]
    fn a_goldfish_reads_only_the_project_skill_roots_and_its_own_session() {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let project = base.join("project");
        let global = base.join("home/.agents/skills");
        let session = base.join("sessions/own");
        write(&project.join("a.txt"), "project");
        write(&global.join("egm/resources/r.md"), "skill");
        write(&session.join("notes.txt"), "own");
        write(&base.join("sessions/parent/events.jsonl"), "parent");
        write(&base.join("claude-homes/parent/x"), "claude");
        write(&base.join("pi-homes/parent/x"), "pi");
        write(&base.join("home/.ssh/id"), "ssh");
        symlink(base.join("home/.ssh/id"), project.join("link")).unwrap();
        let jail = ProjectJail::new(&project).unwrap().with_extra_root(&global);
        for ok in [
            project.join("a.txt"),
            global.join("egm/resources/r.md"),
            session.join("notes.txt"),
        ] {
            assert!(
                jail.resolve_goldfish(&text(&ok), &session).is_ok(),
                "{}",
                ok.display()
            );
        }
        for bad in [
            base.join("sessions/parent/events.jsonl"),
            base.join("claude-homes/parent/x"),
            base.join("pi-homes/parent/x"),
            base.join("home/.ssh/id"),
            project.join("link"),
        ] {
            let error = jail
                .resolve_goldfish(&text(&bad), &session)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("outside the project root, the skill roots and this Session"),
                "{}: {error}",
                bad.display()
            );
        }
        // A relative path out of the own session is judged by its target, not its spelling.
        assert!(jail
            .resolve_goldfish("../parent/events.jsonl", &session)
            .is_err());
    }
}
