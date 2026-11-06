#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
pub mod background;
pub mod bash;
pub mod edit;
pub mod grep;
pub mod jail;
pub mod mcp;
pub mod overflow;
pub mod read;
pub mod seen;
pub mod tag;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

pub use jail::ProjectJail;
pub use mcp::McpServer;
pub use overflow::OverflowStore;
pub use seen::SeenLines;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl ToolError {
    pub fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

pub type ToolResult<T> = std::result::Result<T, ToolError>;

#[derive(Clone, Debug)]
pub struct ToolConfig {
    pub root: PathBuf,
    pub spill_dir: PathBuf,
    /// Rendered tool output above this many bytes spills to `spill_dir`.
    pub overflow_threshold: usize,
    /// `read` refuses files larger than this.
    pub max_read_bytes: usize,
    /// Foreground `bash` output is truncated at this many bytes.
    pub max_output_bytes: usize,
}

impl ToolConfig {
    pub fn new(root: impl Into<PathBuf>, spill_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            spill_dir: spill_dir.into(),
            overflow_threshold: 10 * 1024,
            max_read_bytes: 2 * 1024 * 1024,
            max_output_bytes: 1024 * 1024,
        }
    }
}

pub struct Toolbox {
    config: ToolConfig,
    jail: ProjectJail,
    seen: Arc<SeenLines>,
    overflow: OverflowStore,
    background: background::BackgroundManager,
}

impl Toolbox {
    pub fn new(config: ToolConfig) -> io::Result<Self> {
        let spill_dir = config.spill_dir.clone();
        let seen_dir = spill_dir
            .parent()
            .map(|session| session.join("seen"))
            .unwrap_or_else(|| spill_dir.join("seen"));
        let overflow = OverflowStore::new(spill_dir.clone(), config.overflow_threshold);
        let jail = ProjectJail::new(&config.root)?;
        let background = background::BackgroundManager::new(spill_dir, &config.root);
        Ok(Self {
            background,
            config,
            jail,
            seen: Arc::new(SeenLines::open(seen_dir)),
            overflow,
        })
    }

    /// Hand background exit notices to the daemon by appending them to the
    /// Session's notice file. Draining here makes the hand-off exactly once.
    pub async fn flush_notices(&self) -> io::Result<()> {
        let notices = self.background.take_notices().await;
        if notices.is_empty() {
            return Ok(());
        }
        let path = self
            .config
            .spill_dir
            .parent()
            .map(|session| session.join("notices.jsonl"))
            .unwrap_or_else(|| self.config.spill_dir.join("notices.jsonl"));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        for notice in notices {
            let line = serde_json::json!({ "text": notice });
            std::io::Write::write_all(&mut file, format!("{line}\n").as_bytes())?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        self.jail.root()
    }

    pub fn spill_dir(&self) -> &Path {
        &self.config.spill_dir
    }

    pub fn seen(&self) -> &SeenLines {
        &self.seen
    }

    pub fn jail(&self) -> &ProjectJail {
        &self.jail
    }

    pub fn background(&self) -> &background::BackgroundManager {
        &self.background
    }

    pub fn max_read_bytes(&self) -> usize {
        self.config.max_read_bytes
    }

    pub fn max_output_bytes(&self) -> usize {
        self.config.max_output_bytes
    }

    pub async fn call(&self, name: &str, args: &Value) -> ToolResult<String> {
        match name {
            "read" => read::run(self, args),
            "grep" => grep::run(self, args),
            "edit" => edit::run(self, args),
            "bash" => bash::run(self, args).await,
            "background" => self.background.run(args).await,
            other => Err(ToolError::msg(format!("unknown tool {other:?}"))),
        }
    }

    pub(crate) fn maybe_spill(&self, content: String, origin: &str) -> ToolResult<String> {
        Ok(self.overflow.maybe_spill(content, origin)?)
    }
}
