use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{ToolError, ToolResult};

/// Resolves every tool path inside the project root after symlink resolution.
#[derive(Clone, Debug)]
pub struct ProjectJail {
    root: PathBuf,
    /// Set by `with_extra_root`; `None` keeps reads inside the project root.
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
        if raw.trim().is_empty() {
            return Err(ToolError::msg("path is required"));
        }
        let given = Path::new(raw);
        let candidate = if given.is_absolute() {
            given.to_path_buf()
        } else {
            self.root.join(given)
        };
        let resolved = fs::canonicalize(&candidate)
            .map_err(|e| ToolError::msg(format!("{}: {e}", candidate.display())))?;
        if !self.allows(&resolved) {
            return Err(ToolError::msg(format!(
                "{} is outside the project root {}",
                resolved.display(),
                self.root.display()
            )));
        }
        Ok(resolved)
    }
    pub fn resolve_writable(&self, raw: &str) -> ToolResult<PathBuf> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ToolError::msg("path is required"));
        }
        let given = Path::new(trimmed);
        let candidate = if given.is_absolute() {
            given.to_path_buf()
        } else {
            self.root.join(given)
        };
        let normalized = normalize_lexical(&candidate);
        if !normalized.starts_with(&self.root) {
            return Err(ToolError::msg(format!(
                "{} is outside the project root {}",
                normalized.display(),
                self.root.display()
            )));
        }
        let mut existing = normalized.as_path();
        let mut tail = Vec::new();
        while !existing.exists() {
            if let Some(name) = existing.file_name() {
                tail.push(name.to_os_string());
            }
            let Some(parent) = existing.parent() else {
                break;
            };
            existing = parent;
        }
        let mut resolved = fs::canonicalize(existing)
            .map_err(|e| ToolError::msg(format!("{}: {e}", existing.display())))?;
        if !resolved.starts_with(&self.root) {
            return Err(ToolError::msg(format!(
                "{} is outside the project root {}",
                resolved.display(),
                self.root.display()
            )));
        }
        for part in tail.into_iter().rev() {
            resolved.push(part);
        }
        Ok(resolved)
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
}
