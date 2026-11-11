#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
pub mod background;
pub mod bash;
mod bm25;
pub mod browser;
pub mod ctxsearch;
pub mod edit;
pub mod execute;
pub mod figcraft;
pub mod find;
pub mod graph;
pub mod grep;
pub mod intercept;
pub mod jail;
pub mod mcp;
pub mod mcpclient;
pub mod overflow;
pub mod promptpropose;
pub mod read;
pub mod readurl;
pub mod seen;
pub mod skillmanage;
pub mod skills;
pub mod skillsearch;
mod skillstore;
pub mod skillview;
pub mod subagents;
pub mod tag;
pub mod todo;
pub mod websearch;
pub mod write;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

tokio::task_local! {
    /// Set while a tool call runs for an `execute` script.
    pub(crate) static IN_SCRIPT: ();
}

pub use jail::ProjectJail;
pub use mcp::McpServer;
pub use mcpclient::McpTool;
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
    /// Sourced by every shell, so a new `PATH` entry applies to the next command without a
    /// rebuild or a daemon restart. Tool keys do not live here.
    pub env_file: Option<PathBuf>,
    /// The JSON file Settings writes tool keys into; the tools read it on every call.
    pub tool_keys_file: Option<PathBuf>,
    /// `<config root>/mcp.json`: the stdio MCP servers `execute` can reach.
    pub mcp_file: Option<PathBuf>,
    /// The global skills root (`~/.agents/skills`), readable beyond the project root.
    pub global_skills: Option<PathBuf>,
    /// `<config root>/prompts`: where `prompt_propose` stages proposals beside it.
    pub prompts_dir: Option<PathBuf>,
    pub session_url: Option<String>,
    pub reviewer: bool,
    /// False when the Session's model takes no image input: `read` then refuses image files.
    pub image_input: bool,
    /// A harness cuts an MCP call that blocks longer (Antigravity: 3 minutes) and then dies, so
    /// every blocking tool wait is clamped to this many seconds. `None` keeps each tool's own cap.
    pub max_call_secs: Option<u64>,
    /// Directories searched for a native codebase-memory-mcp before the npx fallback runs.
    pub bin_dirs: Vec<PathBuf>,
}
impl ToolConfig {
    pub fn new(root: impl Into<PathBuf>, spill_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            spill_dir: spill_dir.into(),
            overflow_threshold: 10 * 1024,
            max_read_bytes: 2 * 1024 * 1024,
            max_output_bytes: 1024 * 1024,
            env_file: default_env_file(),
            tool_keys_file: None,
            mcp_file: None,
            global_skills: None,
            prompts_dir: None,
            session_url: None,
            reviewer: false,
            max_call_secs: None,
            image_input: true,
            bin_dirs: graph::default_bin_dirs(),
        }
    }
}

/// `$XDG_CONFIG_HOME/emberloop-rs/env`, or `~/.config/emberloop-rs/env`: the User's runtime
/// environment. The daemon names `<config root>/env` explicitly, because a Session's `HOME` is a
/// harness pi-home.
fn default_env_file() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("emberloop-rs").join("env"))
}

#[derive(Clone)]
pub struct Toolbox {
    config: ToolConfig,
    jail: ProjectJail,
    seen: Arc<SeenLines>,
    overflow: Arc<OverflowStore>,
    background: Arc<background::BackgroundManager>,
    mcp: Arc<mcpclient::McpRegistry>,
}
impl Toolbox {
    pub fn new(config: ToolConfig) -> io::Result<Self> {
        let spill_dir = config.spill_dir.clone();
        let seen_dir = spill_dir
            .parent()
            .map(|session| session.join("seen"))
            .unwrap_or_else(|| spill_dir.join("seen"));
        let overflow = Arc::new(OverflowStore::new(
            spill_dir.clone(),
            config.overflow_threshold,
        ));
        let mut jail = ProjectJail::new(&config.root)?;
        if let Some(global) = &config.global_skills {
            jail = jail.with_extra_root(global);
        }
        let background = Arc::new(background::BackgroundManager::new(spill_dir, &config.root));
        let mcp = Arc::new(mcpclient::McpRegistry::load_with(
            &config.root,
            config.mcp_file.as_deref(),
            &config.bin_dirs,
        ));
        Ok(Self {
            background,
            config,
            jail,
            seen: Arc::new(SeenLines::open(seen_dir)),
            overflow,
            mcp,
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

    pub fn global_skills(&self) -> Option<&Path> {
        self.config.global_skills.as_deref()
    }

    pub fn prompts_dir(&self) -> Option<&Path> {
        self.config.prompts_dir.as_deref()
    }

    /// The longest a blocking call may wait: `max`, lowered to the harness cap.
    pub(crate) fn cap_secs(&self, max: u64) -> u64 {
        self.config
            .max_call_secs
            .map_or(max, |cap| cap.min(max))
            .max(1)
    }

    /// `requested` seconds (else `default`), at least 1, at most `max` and the harness cap.
    pub fn call_secs(&self, requested: Option<u64>, default: u64, max: u64) -> u64 {
        requested.unwrap_or(default).clamp(1, self.cap_secs(max))
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
    pub fn image_input(&self) -> bool {
        self.config.image_input
    }

    pub fn max_read_bytes(&self) -> usize {
        self.config.max_read_bytes
    }

    /// tool_key reads a stored tool key from the file Settings writes, so a key saved in the UI
    /// reaches the next tool call without a shell, an export or a restart.
    pub fn tool_key(&self, variable: &str) -> Option<String> {
        let path = self.config.tool_keys_file.as_ref()?;
        let bytes = std::fs::read(path).ok()?;
        let keys: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&bytes).ok()?;
        keys.get(variable)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    /// The MCP tools `execute` exposes; connects the servers on first use.
    pub async fn mcp_tools(&self) -> ToolResult<Vec<McpTool>> {
        self.mcp.tools().await
    }

    /// Like `mcp_tools`, but gives up waiting after a few seconds so a slow
    /// server start (cold `npx`) never stalls a script; the start goes on in the background.
    pub async fn mcp_tools_soon(&self) -> Vec<McpTool> {
        let mcp = Arc::clone(&self.mcp);
        let start = tokio::spawn(async move { mcp.tools().await });
        match tokio::time::timeout(std::time::Duration::from_millis(1500), start).await {
            Ok(Ok(Ok(tools))) => tools,
            _ => Vec::new(),
        }
    }

    /// The shell prelude that loads the runtime env file and resolves the browser. Empty only
    /// when there is neither.
    pub fn env_prelude(&self) -> String {
        let mut prelude = String::new();
        if let Some(path) = &self.config.env_file {
            let path = path.display().to_string().replace('\'', "'\\''");
            prelude.push_str(&format!("[ -f '{path}' ] && . '{path}'\n"));
        }
        prelude.push_str(&browser::env_prelude());
        prelude
    }

    pub fn max_output_bytes(&self) -> usize {
        self.config.max_output_bytes
    }

    pub async fn call(&self, name: &str, args: &Value) -> ToolResult<String> {
        match name {
            "execute" => execute::run(Arc::new(self.clone()), args).await,
            other => self.call_inner(other, args).await,
        }
    }

    pub(crate) fn tool_available(&self, name: &str) -> bool {
        match name {
            "edit" => !self.config.reviewer,
            "spawn" | "steer" | "stop" | "result" | "submit" => self.config.session_url.is_some(),
            _ => true,
        }
    }

    pub async fn call_inner(&self, name: &str, args: &Value) -> ToolResult<String> {
        if name == "edit" && self.config.reviewer {
            return Err(ToolError::msg("edit is unavailable to reviewer Sessions"));
        }
        match name {
            "read" => {
                if args
                    .get("path")
                    .and_then(Value::as_str)
                    .is_some_and(readurl::is_url_target)
                {
                    readurl::run(self, args).await
                } else {
                    read::run(self, args)
                }
            }
            "todo" => todo::run(self, args),
            "grep" => grep::run(self, args),
            "ctx_search" => ctxsearch::run(self, args),
            "skill_search" => skillsearch::run(self, args),
            "skill_manage" => skillmanage::run(self, args),
            "prompt_propose" => promptpropose::run(self, args),
            "find" => find::run(self, args),
            "edit" => edit::run(self, args),
            "write" => write::run(self, args),
            "bash" => bash::run(self, args).await,
            "background" => {
                self.background
                    .run_capped(args, self.cap_secs(background::MAX_WAIT_SECS))
                    .await
            }
            "websearch" => websearch::run(self, args).await,
            "spawn" | "steer" | "stop" | "result" | "submit" => {
                let url = self
                    .config
                    .session_url
                    .as_deref()
                    .ok_or_else(|| ToolError::msg("Reviewer tools require a daemon Session"))?;
                let text = subagents::run(url, name, args).await?;
                self.maybe_spill(text, name)
            }
            other if other.contains('.') => self.mcp.call(other, args).await,
            other => Err(ToolError::msg(format!("unknown tool {other:?}"))),
        }
    }

    pub(crate) fn maybe_spill(&self, content: String, origin: &str) -> ToolResult<String> {
        // A script inside `execute` filters its own data; a spill stub would hide the content.
        if IN_SCRIPT.try_with(|_| ()).is_ok() {
            return Ok(content);
        }
        Ok(self.overflow.maybe_spill(content, origin)?)
    }
}
