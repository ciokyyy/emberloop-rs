#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
pub mod background;
pub mod bash;
mod bm25;
pub mod browser;
pub mod ctxsearch;
pub mod docs;
pub mod edit;
pub mod execute;
pub mod failedcalls;
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
use std::collections::BTreeSet;
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
    pub subagent_role: Option<String>,
    /// False when the Session's model takes no image input: `read` then refuses image files.
    pub image_input: bool,
    /// A harness cuts an MCP call that blocks longer (Antigravity: 3 minutes) and then dies, so
    /// every blocking tool wait is clamped to this many seconds. `None` keeps each tool's own cap.
    pub max_call_secs: Option<u64>,
    pub bin_dirs: Vec<PathBuf>,
    /// Whether oversized tool output spills to `ctx:` handles and `ctx_search` is available.
    pub ctx_spill_enabled: bool,
    /// Record MCP tool results into `spill_dir/mcp_outputs/` for harnesses (like agy) that drop
    /// live tool outputs on the wire.
    pub capture_mcp_output: bool,
    /// Tool names whose failures persist under `spill_dir/failed_calls/` for `retry_call`.
    pub retry_allowlist: BTreeSet<String>,
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
            subagent_role: None,
            max_call_secs: None,
            image_input: true,
            bin_dirs: graph::default_bin_dirs(),
            ctx_spill_enabled: false,
            capture_mcp_output: false,
            retry_allowlist: ["spawn", "steer"].into_iter().map(str::to_string).collect(),
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
    failed_calls: failedcalls::FailedCallStore,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum SubagentKind {
    None,
    Reviewer,
    Scout,
    Worker,
}

impl Toolbox {
    pub fn new(config: ToolConfig) -> io::Result<Self> {
        let spill_dir = config.spill_dir.clone();
        let failed_calls = failedcalls::FailedCallStore::new(&spill_dir);
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
            failed_calls,
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
    pub fn failed_calls(&self) -> &failedcalls::FailedCallStore {
        &self.failed_calls
    }

    pub fn image_input(&self) -> bool {
        self.config.image_input
    }

    pub fn capture_mcp_output(&self) -> bool {
        self.config.capture_mcp_output
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
            prelude.push_str(&format!(
                "export EMBERLOOP_ENV_FILE='{path}'\n[ -f '{path}' ] && . '{path}'\n"
            ));
        }
        prelude.push_str(&browser::env_prelude());
        prelude
    }

    pub fn max_output_bytes(&self) -> usize {
        self.config.max_output_bytes
    }

    pub async fn call(&self, name: &str, args: &Value) -> ToolResult<String> {
        match name {
            "execute" => {
                let result = execute::run(Arc::new(self.clone()), args).await;
                self.wrap_retry_error(name, args, result)
            }
            other => self.call_inner(other, args).await,
        }
    }
    fn subagent_kind(&self) -> SubagentKind {
        match self.config.subagent_role.as_deref() {
            Some(role) if emberloop_core::steer::is_reviewer_role(role) => SubagentKind::Reviewer,
            Some("scout") => SubagentKind::Scout,
            Some(_) => SubagentKind::Worker,
            None if self.config.reviewer => SubagentKind::Reviewer,
            None => SubagentKind::None,
        }
    }

    pub(crate) fn is_scout(&self) -> bool {
        self.subagent_kind() == SubagentKind::Scout
    }

    fn denied_reason(&self, name: &str) -> Option<String> {
        match (self.subagent_kind(), name) {
            (SubagentKind::Scout, "edit" | "write") => {
                Some(format!("{name} is unavailable to scout Sessions"))
            }
            (SubagentKind::Reviewer, "edit") => {
                Some("edit is unavailable to reviewer Sessions".into())
            }
            (
                SubagentKind::Reviewer | SubagentKind::Scout | SubagentKind::Worker,
                "spawn" | "steer" | "stop" | "result",
            ) => Some(format!("{name} is unavailable to subagent Sessions")),
            _ => None,
        }
    }

    pub(crate) fn tool_available(&self, name: &str) -> bool {
        if self.denied_reason(name).is_some() {
            return false;
        }
        match name {
            "ctx_search" => self.config.ctx_spill_enabled,
            "spawn" | "steer" | "stop" | "result" | "submit" => self.config.session_url.is_some(),
            _ => true,
        }
    }

    pub async fn call_inner(&self, name: &str, args: &Value) -> ToolResult<String> {
        if let Some(reason) = self.denied_reason(name) {
            return Err(ToolError::msg(reason));
        }
        let result = self.dispatch_inner(name, args).await;
        self.wrap_retry_error(name, args, result)
    }

    async fn dispatch_inner(&self, name: &str, args: &Value) -> ToolResult<String> {
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
            "ctx_search" => {
                if !self.config.ctx_spill_enabled {
                    return Err(ToolError::msg("ctx_search is currently disabled"));
                }
                ctxsearch::run(self, args)
            }
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
            "retry_call" => failedcalls::run_retry(self, args).await,
            "spawn" | "steer" | "stop" | "result" | "submit" => {
                let url = self
                    .config
                    .session_url
                    .as_deref()
                    .ok_or_else(|| ToolError::msg("Subagent tools require a daemon Session"))?;
                let text = subagents::run(url, name, args).await?;
                self.maybe_spill(text, name)
            }
            other if other.contains('.') => self.mcp.call(other, args).await,
            other => Err(ToolError::msg(format!("unknown tool {other:?}"))),
        }
    }
    fn wrap_retry_error(
        &self,
        name: &str,
        args: &Value,
        result: ToolResult<String>,
    ) -> ToolResult<String> {
        let Err(error) = result else {
            return result;
        };
        if name == "retry_call" || !self.config.retry_allowlist.contains(name) {
            return Err(error);
        }
        match self.failed_calls.record(name, args) {
            Ok(id) => Err(ToolError::msg(format!(
                "{error}\n\n[failed call #{id}] Retry with retry_call(id={id}, ...)"
            ))),
            Err(record_err) => {
                tracing::warn!(tool = name, error = %record_err, "failed to record failed call for retry");
                Err(error)
            }
        }
    }
    pub(crate) fn maybe_spill(&self, content: String, origin: &str) -> ToolResult<String> {
        if !self.config.ctx_spill_enabled {
            return Ok(content);
        }
        // A script inside `execute` filters its own data; a spill stub would hide the content.
        if IN_SCRIPT.try_with(|_| ()).is_ok() {
            return Ok(content);
        }
        Ok(self.overflow.maybe_spill(content, origin)?)
    }
}
