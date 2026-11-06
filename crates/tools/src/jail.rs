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
}
