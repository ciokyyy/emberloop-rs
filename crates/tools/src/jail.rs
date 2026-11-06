use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{ToolError, ToolResult};

/// Resolves every tool path inside the project root after symlink resolution.
#[derive(Clone, Debug)]
pub struct ProjectJail {
    root: PathBuf,
}

impl ProjectJail {
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = fs::canonicalize(root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
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
        if !resolved.starts_with(&self.root) {
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
