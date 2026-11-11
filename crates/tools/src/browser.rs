//! The Chrome that `agent-browser` must use, exported into every shell.
//!
//! A Session's `HOME` is a fresh pi-home, so `agent-browser` looks for its browser in an empty
//! cache, finds nothing, and every command fails with "Chrome not found" until the model gives up.
//! Resolving one executable once and exporting it makes the browser work on the first call.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Every shell gets a heap cap, so a page with a reactive loop hits `--max-old-space-size`
/// instead of growing until the machine swaps.
const DEFAULT_ARGS: &str = "--js-flags=--max-old-space-size=512 --disable-dev-shm-usage";

/// The daemon closes an idle browser after this long, so a model that forgets `close` still frees
/// the ~1.4 GB of Chromium processes. Verified: the whole process tree is gone seconds after the
/// last command.
const DEFAULT_IDLE_MS: &str = "300000";

/// Shell prelude lines. The chrome line is omitted when nothing is found, leaving
/// `agent-browser install` as the User's fallback.
pub fn env_prelude() -> String {
    let mut out = String::new();
    if let Some(path) = executable() {
        out.push_str(&format!(
            ": \"${{AGENT_BROWSER_EXECUTABLE_PATH:={}}}\"; export AGENT_BROWSER_EXECUTABLE_PATH\n",
            escaped(&path.display().to_string())
        ));
    }
    out.push_str(&format!(
        ": \"${{AGENT_BROWSER_ARGS:={DEFAULT_ARGS}}}\"; export AGENT_BROWSER_ARGS\n"
    ));
    out.push_str(&format!(
        ": \"${{AGENT_BROWSER_IDLE_TIMEOUT_MS:={DEFAULT_IDLE_MS}}}\"; export AGENT_BROWSER_IDLE_TIMEOUT_MS\n"
    ));
    out
}

/// Inside the prelude's double quotes a path needs no quoting of its own; `agent-browser` wants a
/// plain path, so single quotes here would become part of the value.
fn escaped(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The one browser path every shell reuses.
pub fn executable() -> Option<&'static Path> {
    // shortcut: resolved once per tool server, so a browser installed mid-Session is picked up on
    // the next Session only; re-resolve when that annoys someone.
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND.get_or_init(resolve).as_deref()
}

fn resolve() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("AGENT_BROWSER_EXECUTABLE_PATH").map(PathBuf::from) {
        if explicit.is_file() {
            return Some(explicit);
        }
    }
    caches()
        .iter()
        .find_map(|cache| playwright_in(cache))
        .or_else(|| homes().iter().find_map(|home| agent_browser_in(home)))
        .or_else(system_chrome)
}

/// Cache roots that hold Playwright's Chromium.
fn caches() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        out.push(PathBuf::from(xdg));
    }
    let mut homes = homes();
    homes.dedup();
    out.extend(homes.into_iter().map(|home| home.join(".cache")));
    out
}

/// The real home plus the Session home, plus the sibling homes beside it: a browser another
/// Session downloaded is a browser this Session does not have to download again.
fn homes() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(home) = real_home() {
        out.push(home);
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if !out.contains(&home) {
            out.push(home.clone());
        }
        // `<data root>/pi-homes/<session>`
        if let Some(parent) = home.parent() {
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let dir = entry.path();
                    if dir != home && dir.is_dir() {
                        out.push(dir);
                    }
                }
            }
        }
    }
    out
}

fn real_home() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = std::fs::metadata("/proc/self").ok()?.uid();
        let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
        if let Some(home) = home_for_uid(&passwd, uid) {
            return Some(home);
        }
    }
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `name:passwd:uid:gid:gecos:home:shell`
fn home_for_uid(passwd: &str, uid: u32) -> Option<PathBuf> {
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let home = fields.get(5)?;
        (fields.get(2)?.parse::<u32>().ok() == Some(uid) && !home.is_empty())
            .then(|| PathBuf::from(home))
    })
}

/// `<cache>/ms-playwright/chromium-<version>/chrome-linux{64,}/chrome`, newest version first.
fn playwright_in(cache: &Path) -> Option<PathBuf> {
    let root = cache.join("ms-playwright");
    for dir in newest(&root, "chromium-")? {
        for sub in ["chrome-linux64", "chrome-linux"] {
            let exe = dir.join(sub).join("chrome");
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

/// `<home>/.agent-browser/browsers/chrome-<version>/chrome`, newest version first.
fn agent_browser_in(home: &Path) -> Option<PathBuf> {
    let root = home.join(".agent-browser").join("browsers");
    newest(&root, "chrome-")?
        .into_iter()
        .map(|dir| dir.join("chrome"))
        .find(|exe| exe.is_file())
}

fn system_chrome() -> Option<PathBuf> {
    const NAMES: [&str; 5] = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
    ];
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        NAMES
            .iter()
            .map(|name| dir.join(name))
            .find(|exe| exe.is_file())
    })
}

/// Directories under `root` named `<prefix><version>`, newest version first.
fn newest(root: &Path, prefix: &str) -> Option<Vec<PathBuf>> {
    let mut found: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let key = version_key(name.strip_prefix(prefix)?);
            Some((key, entry.path()))
        })
        .collect();
    if found.is_empty() {
        return None;
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    Some(found.into_iter().map(|(_, path)| path).collect())
}

fn version_key(s: &str) -> Vec<u64> {
    s.split(|c: char| !c.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn picks_the_newest_playwright_chromium() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path();
        touch(&cache.join("ms-playwright/chromium-100/chrome-linux64/chrome"));
        touch(&cache.join("ms-playwright/chromium-1254/chrome-linux/chrome"));
        touch(&cache.join("ms-playwright/chromium_headless_shell-1254/shell"));
        assert!(playwright_in(cache)
            .unwrap()
            .ends_with("chromium-1254/chrome-linux/chrome"));
    }

    #[test]
    fn picks_the_newest_agent_browser_chrome() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path()
                .join(".agent-browser/browsers/chrome-150.0.1/chrome"),
        );
        touch(
            &dir.path()
                .join(".agent-browser/browsers/chrome-157.0.8103.38/chrome"),
        );
        assert!(agent_browser_in(dir.path())
            .unwrap()
            .ends_with("chrome-157.0.8103.38/chrome"));
    }

    #[test]
    fn sorts_versions_numerically_not_lexically() {
        assert!(version_key("9.2.1") < version_key("10.0.0"));
    }

    #[test]
    fn the_prelude_caps_the_heap_and_closes_an_idle_browser() {
        let prelude = env_prelude();
        assert!(prelude.contains("AGENT_BROWSER_ARGS:="));
        assert!(prelude.contains("max-old-space-size=512"));
        assert!(prelude.contains("AGENT_BROWSER_IDLE_TIMEOUT_MS:=300000"));
    }

    #[test]
    fn reads_the_home_field_of_the_matching_uid() {
        let passwd =
            "root:x:0:0:root:/root:/bin/bash\ndaemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin";
        assert_eq!(home_for_uid(passwd, 0).unwrap(), Path::new("/root"));
        assert!(home_for_uid(passwd, 99).is_none());
    }
}
