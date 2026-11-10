#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! The real `claude` binary over newline-delimited JSON on its stdin and stdout:
//! no ACP adapter, no Node, no SDK crate between the daemon and `claude`.
pub mod parser;

use async_trait::async_trait;
use harness::{
    EventStream, Harness, HarnessError, HarnessEvent, HarnessSession, HarnessWiring, PromptImage,
};
use parser::Output;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{timeout, Duration};

/// A CLI that never reaches `system/init` is a broken launch, not a slow one.
const INIT_TIMEOUT: Duration = Duration::from_secs(30);
/// A killed CLI stops writing to stderr within moments; its readers never delay a turn for long.
const STDERR_DRAIN: Duration = Duration::from_millis(500);
/// The thinking levels `--effort` accepts; `off` and anything else means no flag.
const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
/// Every built-in the recorded `system/init` lists, plus the ones older CLIs had.
/// `--tools ""` already disables them; the deny list keeps a CLI that ignores it honest.
const DENIED_TOOLS: &[&str] = &[
    "Bash",
    "CronCreate",
    "CronDelete",
    "CronList",
    "DesignSync",
    "Edit",
    "EnterWorktree",
    "ExitWorktree",
    "Glob",
    "Grep",
    "ListAgents",
    "Monitor",
    "NotebookEdit",
    "Read",
    "ReportFindings",
    "ScheduleWakeup",
    "SendMessage",
    "Skill",
    "Task",
    "TaskStop",
    "TodoWrite",
    "ToolSearch",
    "WebFetch",
    "WebSearch",
    "Workflow",
    "Write",
];

/// Inline settings passed with `--settings`; the CLI applies them above the user file.
/// The away recap is a background model call, so it stays off.
const CLI_INLINE_SETTINGS: &str = r#"{"awaySummaryEnabled":false}"#;
/// Every variable the child env carries besides the paths, proxy and credentials.
/// Each one stops a feature that adds text to the model context, makes an extra model
/// call, or sends background traffic; the proxy owns the context, so none of them may run.
const CLI_DISABLING_ENV: &[(&str, &str)] = &[
    // Load every MCP tool up front: deferred tools cost a ToolSearch round trip (a model call).
    ("ENABLE_TOOL_SEARCH", "false"),
    // No memory files are read or written into the context.
    ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1"),
    // CLAUDE.md files (user, project, auto memory) are never injected: the proxy owns the policy text.
    ("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1"),
    // Drops the built-in commit and PR instructions and the git status snapshot.
    ("CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS", "1"),
    // Not NONESSENTIAL_TRAFFIC: it would also stop the CLI's auto-update, which stays on.
    // Feedback surveys, the /feedback command and Claude-drafted feedback.
    ("CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY", "1"),
    ("DISABLE_FEEDBACK_COMMAND", "1"),
    ("CLAUDE_CODE_SEND_FEEDBACK", "0"),
    // Feature-flag fetching: code defaults for every flag.
    ("DISABLE_GROWTHBOOK", "1"),
    // No automatic plugin marketplace registration.
    ("CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL", "1"),
    // No run_in_background and no background runs that report back in a later turn.
    ("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1"),
    // A slow MCP call is never moved to a background task that reports back later.
    ("CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS", "0"),
    // No session recap model call when the user comes back to the terminal.
    ("CLAUDE_CODE_ENABLE_AWAY_SUMMARY", "0"),
    // Prompt suggestions are a model call per turn; the CLI form of the SDK promptSuggestions=false.
    ("CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION", "false"),
    // Skips the background small-model request that generates a session title.
    ("CLAUDE_CODE_DISABLE_TERMINAL_TITLE", "1"),
    ("DISABLE_TELEMETRY", "1"),
    ("DISABLE_ERROR_REPORTING", "1"),
    // Skill, workflow, agent view and cron listings are not put in the context.
    ("CLAUDE_CODE_DISABLE_BUNDLED_SKILLS", "1"),
    ("CLAUDE_CODE_DISABLE_WORKFLOWS", "1"),
    ("CLAUDE_CODE_DISABLE_AGENT_VIEW", "1"),
    ("CLAUDE_CODE_DISABLE_CRON", "1"),
    // Checkpoints are bookkeeping the stripped tool set cannot use.
    ("CLAUDE_CODE_DISABLE_FILE_CHECKPOINTING", "1"),
    // The proxy owns context: the CLI's own compaction is off (automatic, idle and manual).
    ("DISABLE_COMPACT", "1"),
    ("DISABLE_AUTO_COMPACT", "1"),
    // Stops fetching claude.ai connectors, whose tools would otherwise join the context.
    ("ENABLE_CLAUDEAI_MCP_SERVERS", "false"),
];

/// The hosts the CLI must reach without a proxy: the daemon's own loopback.
const LOOPBACK: &str = "127.0.0.1,localhost";
const DEFAULT_MODEL: &str = "sonnet";
const MCP_SERVER: &str = "el";
/// The Account home the sign-in writes; every Session copies its credential from it.
pub const ACCOUNT_HOME: &str = "default";

/// The real `claude` CLI. Each Session owns one `CLAUDE_CONFIG_DIR` under `homes`,
/// so its login and transcripts stay out of the User's own `~/.claude`.
pub struct ClaudeHarness {
    bin: PathBuf,
    homes: PathBuf,
    default_model: String,
}

impl ClaudeHarness {
    pub fn new(bin: impl Into<PathBuf>, homes: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            homes: homes.into(),
            default_model: DEFAULT_MODEL.to_string(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    /// The catalogue id carries the harness (`claude/opus`); the CLI wants the alias.
    fn model_for(&self, wiring: &HarnessWiring) -> String {
        wiring
            .model
            .as_deref()
            .map(|model| model.strip_prefix("claude/").unwrap_or(model))
            .filter(|model| !model.is_empty())
            .unwrap_or(&self.default_model)
            .to_string()
    }

    /// The complete argument list; one builder serves a new and a resumed Session.
    fn launch(&self, session_id: &str, wiring: &HarnessWiring) -> Result<Launch, HarnessError> {
        let config_dir = self.homes.join(session_id);
        std::fs::create_dir_all(&config_dir)?;
        let cwd = match &wiring.workdir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                dir.clone()
            }
            None => {
                let dir = config_dir.join("agent-work");
                std::fs::create_dir_all(&dir)?;
                dir
            }
        };
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--replay-user-messages",
            "--setting-sources",
            "user",
            "--permission-mode",
            "dontAsk",
            "--permission-prompts",
            "none",
            "--allowedTools",
            "mcp__el",
            "--tools",
            "",
            "--disallowedTools",
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
        args.push(DENIED_TOOLS.join(","));
        args.push("--settings".into());
        args.push(CLI_INLINE_SETTINGS.into());
        args.push("--model".into());
        args.push(self.model_for(wiring));
        if let Some(effort) = wiring
            .thought_level
            .as_deref()
            .filter(|level| EFFORTS.contains(level))
        {
            args.push("--effort".into());
            args.push(effort.to_string());
        }
        if let Some(config) = mcp_config(wiring) {
            args.push("--strict-mcp-config".into());
            args.push("--mcp-config".into());
            args.push(config);
        }
        let mut env = vec![(
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )];
        // The owned config dir is the login; nothing of the User's home is read.
        env.push(("HOME".into(), config_dir.display().to_string()));
        env.push(("CLAUDE_CONFIG_DIR".into(), config_dir.display().to_string()));
        // A long-lived `claude setup-token` token never refreshes, so sessions cannot race each other
        // on the rotating refresh token (the cause of cascading logouts). Absent: the login file.
        if let Ok(token) =
            std::fs::read_to_string(self.homes.join(ACCOUNT_HOME).join("oauth-token"))
        {
            if !token.trim().is_empty() {
                env.push(("CLAUDE_CODE_OAUTH_TOKEN".into(), token.trim().into()));
            }
        }
        // The proxy's `claude` route forwards the CLI's own Authorization header upstream.
        env.push((
            "ANTHROPIC_BASE_URL".into(),
            format!("{}/claude", wiring.proxy_url.trim_end_matches('/')),
        ));
        // The proxy is loopback; an inherited proxy setting must never carry it.
        for key in ["NO_PROXY", "no_proxy"] {
            env.push((key.into(), LOOPBACK.into()));
        }
        for (key, value) in CLI_DISABLING_ENV {
            env.push(((*key).into(), (*value).into()));
        }
        for (key, value) in &wiring.env {
            env.push((key.clone(), value.clone()));
        }
        Ok(Launch {
            program: self.bin.clone(),
            args,
            env,
            cwd,
            sync: Syncer {
                account_home: self.homes.join(ACCOUNT_HOME),
                config_dir: config_dir.clone(),
            },
            config_dir,
        })
    }
}

struct Launch {
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cwd: PathBuf,
    config_dir: PathBuf,
    sync: Syncer,
}

/// The Account home and one Session home, whose credentials are kept in step.
#[derive(Clone)]
struct Syncer {
    account_home: PathBuf,
    config_dir: PathBuf,
}

impl Syncer {
    /// The file work is blocking, so it runs off the async workers.
    async fn run(&self) {
        let syncer = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            sync_account(&syncer.account_home, &syncer.config_dir);
        })
        .await;
    }
}

const CREDENTIALS: &str = ".credentials.json";
const CLAUDE_JSON: &str = ".claude.json";

/// Keeps the Account's credential and this Session's in step. The CLI rotates OAuth refresh
/// tokens, so the copy that expires later is the live one and is copied over the other.
/// `.claude.json` is only seeded. Best effort: a failure leaves the files as they were.
pub fn sync_account(account_home: &Path, config_dir: &Path) {
    sync_credentials(
        &account_home.join(CREDENTIALS),
        &config_dir.join(CREDENTIALS),
    );
    let claude_json = config_dir.join(CLAUDE_JSON);
    if !claude_json.exists() {
        let _ = std::fs::copy(account_home.join(CLAUDE_JSON), &claude_json);
    }
}

fn sync_credentials(account: &Path, session: &Path) {
    let (account_at, session_at) = (expires_at(account), expires_at(session));
    if account_at > session_at {
        let _ = install(account, session);
    } else if session_at > account_at {
        let _ = install(session, account);
    }
}

/// The `claudeAiOauth.expiresAt` of a credential file in ms; 0 when it holds no usable credential.
fn expires_at(path: &Path) -> f64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value["claudeAiOauth"]["expiresAt"].as_f64())
        .filter(|at| at.is_finite() && *at > 0.0)
        .unwrap_or(0.0)
}

/// Copies `from` over `to` through a private temp file beside it, renamed into place, so a
/// reader never sees a half-written credential.
fn install(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = to
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let tmp = to.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let written = copy_private(from, &tmp).and_then(|()| std::fs::rename(&tmp, to));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Created 0600 from the start, so the token is never readable by others, not even briefly.
fn copy_private(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)?;
    std::io::copy(&mut std::fs::File::open(from)?, &mut out)?;
    Ok(())
}

fn mcp_config(wiring: &HarnessWiring) -> Option<String> {
    let server = match (&wiring.mcp_url, &wiring.mcp_command) {
        (Some(url), _) => json!({"type": "http", "url": url, "alwaysLoad": true}),
        (None, Some(command)) if !command.is_empty() => {
            json!({"command": command[0], "args": &command[1..], "alwaysLoad": true})
        }
        _ => return None,
    };
    Some(json!({"mcpServers": {MCP_SERVER: server}}).to_string())
}

/// One user turn: text parts, then image parts, both native to the CLI.
fn user_message(prompt: &str, images: &[PromptImage]) -> String {
    let mut content = vec![json!({"type": "text", "text": prompt})];
    for image in images {
        content.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": image.mime_type,
                "data": image.data_base64,
            },
        }));
    }
    let message = json!({"type": "user", "message": {"role": "user", "content": content}});
    format!("{message}\n")
}

struct Process {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    resume: Option<String>,
    init: Option<oneshot::Receiver<Result<(), String>>>,
    /// Set by this process's stdout reader at EOF, which comes before `try_wait` sees the exit.
    gone: Arc<AtomicBool>,
    /// Set by `cancel` on this process: the CLI may report the interrupted turn as an error and
    /// exit, and the user asked for that. The next message starts a new process (resumed), so the
    /// old one's late error or EOF is still silenced.
    cancelled: Arc<AtomicBool>,
    stderr: Option<tokio::task::JoinHandle<()>>,
}

pub struct ClaudeSession {
    launch: Launch,
    state: Mutex<Process>,
    /// The CLI's own id for this conversation, filled by `system/init`.
    session_id: Arc<StdMutex<Option<String>>>,
    /// The last thing the CLI wrote to stderr: the only place it says why a start failed.
    tail: Arc<StdMutex<String>>,
    /// Messages written that the CLI has not taken yet. One sent mid-turn is folded into the turn
    /// at a tool boundary (one `result`) or waits and runs as a second turn (two), so a `result`
    /// is the end of the work only when nothing is waiting.
    waiting: Arc<AtomicUsize>,
    resumed: AtomicBool,
    tx: mpsc::UnboundedSender<harness::HarnessEvent>,
    rx: StdMutex<Option<mpsc::UnboundedReceiver<harness::HarnessEvent>>>,
}

impl ClaudeSession {
    fn new(launch: Launch, resume: Option<String>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            launch,
            state: Mutex::new(Process {
                child: None,
                stdin: None,
                init: None,
                gone: Arc::new(AtomicBool::new(false)),
                cancelled: Arc::new(AtomicBool::new(false)),
                stderr: None,
                resume: resume.clone(),
            }),
            session_id: Arc::new(StdMutex::new(resume)),
            tail: Arc::new(StdMutex::new(String::new())),
            waiting: Arc::new(AtomicUsize::new(0)),
            resumed: AtomicBool::new(false),
            tx,
            rx: StdMutex::new(Some(rx)),
        }
    }

    // The CLI emits system/init only after its first user message.
    async fn ensure(&self) -> Result<(), HarnessError> {
        let mut state = self.state.lock().await;
        let gone = state.gone.load(Ordering::SeqCst);
        let interrupted = state.cancelled.load(Ordering::SeqCst);
        if let Some(child) = state.child.as_mut() {
            if !gone && !interrupted && child.try_wait()?.is_none() {
                return Ok(());
            }
            // A cancelled process may be on its way out and would swallow the next message, so
            // the conversation resumes in a new one.
            if interrupted {
                let _ = child.kill().await;
            }
            state.child = None;
            state.stdin = None;
        }
        // Every spawn and respawn starts from the fresher of the Account's and the Session's credential.
        self.launch.sync.run().await;
        // A crash inside a turn resumes the conversation this process already had.
        let token = state.resume.clone().or_else(|| {
            self.session_id
                .lock()
                .ok()
                .and_then(|slot| slot.clone())
                .filter(|token| !token.is_empty())
        });
        let mut args = self.launch.args.clone();
        if let Some(token) = &token {
            args.push("--resume".into());
            args.push(token.clone());
        }
        let mut command = Command::new(&self.launch.program);
        command
            .args(&args)
            .current_dir(&self.launch.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .stderr(Stdio::piped())
            // `env_clear` keeps any inherited token or base URL out; `launch.env` is the allow-list.
            .env_clear();
        for (key, value) in &self.launch.env {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .map_err(|error| HarnessError::Process(format!("claude: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| HarnessError::Process("claude: no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Process("claude: no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| HarnessError::Process("claude: no stderr".into()))?;
        let (init_tx, init_rx) = oneshot::channel();
        let gone = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        self.waiting.store(0, Ordering::SeqCst);
        tokio::spawn(read_stream(
            stdout,
            Reader {
                session_id: Arc::clone(&self.session_id),
                tx: self.tx.clone(),
                init: Some(init_tx),
                gone: Arc::clone(&gone),
                waiting: Arc::clone(&self.waiting),
                cancelled: Arc::clone(&cancelled),
                sync: self.launch.sync.clone(),
            },
        ));
        if let Some(task) = state.stderr.take() {
            task.abort();
        }
        state.stderr = Some(tokio::spawn(read_tail(stderr, Arc::clone(&self.tail))));
        state.init = Some(init_rx);
        state.gone = gone;
        state.cancelled = cancelled;
        self.resumed.store(token.is_some(), Ordering::SeqCst);
        state.child = Some(child);
        state.stdin = Some(stdin);
        Ok(())
    }

    async fn write(&self, line: String) -> Result<(), HarnessError> {
        self.ensure().await?;
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let written = self.write_line(line).await;
        if written.is_err() {
            let _ = self
                .waiting
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        }
        written
    }

    async fn write_line(&self, line: String) -> Result<(), HarnessError> {
        let mut state = self.state.lock().await;
        let stdin = state
            .stdin
            .as_mut()
            .ok_or_else(|| HarnessError::Process("claude: stdin closed".into()))?;
        let written = match stdin.write_all(line.as_bytes()).await {
            Ok(()) => stdin.flush().await,
            Err(error) => Err(error),
        };
        if let Err(error) = written {
            if error.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(error.into());
            }
            // The CLI usually says why it is gone before the pipe breaks; that beats the pipe.
            let Some(init) = state.init.take() else {
                return Err(error.into());
            };
            return match timeout(STDERR_DRAIN, init).await {
                Ok(Ok(Err(failure))) => Err(self.with_tail(HarnessError::Protocol(failure))),
                Ok(Err(_)) => {
                    Err(self.with_tail(HarnessError::Process("claude exited before init".into())))
                }
                // Still alive and silent: report the pipe itself.
                Ok(Ok(Ok(()))) | Err(_) => Err(error.into()),
            };
        }
        let Some(init) = state.init.take() else {
            return Ok(());
        };
        let failure = match timeout(INIT_TIMEOUT, init).await {
            Ok(Ok(Ok(()))) => return Ok(()),
            Ok(Ok(Err(error))) => HarnessError::Protocol(error),
            Ok(Err(_)) => HarnessError::Process("claude exited before init".into()),
            Err(_) => HarnessError::Process("claude did not init".into()),
        };
        state.stdin = None;
        if let Some(mut child) = state.child.take() {
            let _ = child.kill().await;
        }
        // The CLI's stderr says why it never started, so drain it before giving up on it.
        if let Some(task) = state.stderr.take() {
            let _ = timeout(STDERR_DRAIN, task).await;
        }
        Err(self.with_tail(failure))
    }

    fn with_tail(&self, error: HarnessError) -> HarnessError {
        let tail = self
            .tail
            .lock()
            .map(|tail| tail.clone())
            .unwrap_or_default();
        if tail.is_empty() {
            error
        } else {
            HarnessError::Process(format!("{error}: {tail}"))
        }
    }
}

/// What the reader shares with its Session and Process, and the credential sync it runs at each turn end.
struct Reader {
    session_id: Arc<StdMutex<Option<String>>>,
    tx: mpsc::UnboundedSender<harness::HarnessEvent>,
    init: Option<oneshot::Sender<Result<(), String>>>,
    gone: Arc<AtomicBool>,
    waiting: Arc<AtomicUsize>,
    cancelled: Arc<AtomicBool>,
    sync: Syncer,
}

/// Every line the CLI prints; `system/init` answers the spawn, the rest becomes events.
async fn read_stream(stdout: tokio::process::ChildStdout, mut reader: Reader) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        for output in parser::parse_line(&line) {
            reader.handle(output).await;
        }
    }
    reader.finish();
}

impl Reader {
    async fn handle(&mut self, output: Output) {
        match output {
            Output::Init(found) => accept_init(found, &mut self.init, &self.session_id),
            Output::Consumed => {
                let _ = self
                    .waiting
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
            }
            Output::Event(HarnessEvent::TurnFinished { .. })
                if self.waiting.load(Ordering::SeqCst) > 0 => {}
            Output::Event(HarnessEvent::Error(_)) if self.cancelled.load(Ordering::SeqCst) => {}
            Output::Event(event) => {
                // Synced before the event goes out, so a consumer of the turn end sees the copies.
                if matches!(event, HarnessEvent::TurnFinished { .. }) {
                    self.sync.run().await;
                }
                let _ = self.tx.send(event);
            }
        }
    }

    /// The stream ended: the process is gone, and a start still waiting for init fails.
    fn finish(mut self) {
        if let Some(tx) = self.init.take() {
            let _ = tx.send(Err("claude exited before init".into()));
        }
        self.gone.store(true, Ordering::SeqCst);
        if !self.cancelled.load(Ordering::SeqCst) {
            let _ = self.tx.send(HarnessEvent::Error("claude exited".into()));
        }
    }
}

/// Keep the last thing the CLI complained about: a failed start has no other witness.
async fn read_tail(stderr: tokio::process::ChildStderr, tail: Arc<StdMutex<String>>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            if let Ok(mut slot) = tail.lock() {
                *slot = line;
            }
        }
    }
}

/// `system/init` answers the spawn and carries the token a crash resumes with.
fn accept_init(
    found: parser::Init,
    init: &mut Option<oneshot::Sender<Result<(), String>>>,
    session_id: &StdMutex<Option<String>>,
) {
    if let Some(id) = found.session_id.clone() {
        if let Ok(mut slot) = session_id.lock() {
            *slot = Some(id);
        }
    }
    let verdict = if found.connected(MCP_SERVER) {
        Ok(())
    } else {
        Err(format!("claude: MCP server {MCP_SERVER} did not connect"))
    };
    if let Some(tx) = init.take() {
        let _ = tx.send(verdict);
    }
}

#[async_trait]
impl Harness for ClaudeHarness {
    async fn spawn(
        &self,
        session_id: &str,
        wiring: HarnessWiring,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        let launch = self.launch(session_id, &wiring)?;
        let session = ClaudeSession::new(launch, wiring.resume_token);
        session.ensure().await?;
        Ok(Box::new(session))
    }
}

#[async_trait]
impl HarnessSession for ClaudeSession {
    async fn send(&self, prompt: &str, images: Vec<PromptImage>) -> Result<(), HarnessError> {
        self.write(user_message(prompt, &images)).await
    }

    fn events(&self) -> EventStream {
        let Ok(mut slot) = self.rx.lock() else {
            return Box::pin(tokio_stream::empty());
        };
        let Some(rx) = slot.take() else {
            return Box::pin(tokio_stream::empty());
        };
        Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
    }

    /// SIGINT ends the turn (documented) and leaves the process usable.
    async fn cancel(&self) -> Result<(), HarnessError> {
        self.waiting.store(0, Ordering::SeqCst);
        let mut state = self.state.lock().await;
        state.cancelled.store(true, Ordering::SeqCst);
        if let Some(pid) = state.child.as_mut().and_then(|child| child.id()) {
            // SAFETY: `pid` is our live child; SIGINT only ends its current turn.
            unsafe {
                libc::kill(pid as i32, libc::SIGINT);
            }
        }
        Ok(())
    }

    /// Kills the CLI and waits for it to exit, so no zombie is left behind.
    async fn close(&self) -> Result<(), HarnessError> {
        let mut state = self.state.lock().await;
        state.stdin = None;
        if let Some(mut child) = state.child.take() {
            let _ = child.kill().await;
        }
        Ok(())
    }

    async fn resume(&self, token: &str) -> Result<(), HarnessError> {
        let mut state = self.state.lock().await;
        state.resume = Some(token.to_string());
        if let Some(mut child) = state.child.take() {
            let _ = child.kill().await;
        }
        state.stdin = None;
        Ok(())
    }

    /// The same stdin write as `send`: the CLI queues it into the running turn.
    async fn wake(&self, text: &str, images: Vec<PromptImage>) -> Result<(), HarnessError> {
        self.write(user_message(text, &images)).await
    }

    fn resume_token(&self) -> Option<String> {
        self.session_id.lock().ok().and_then(|slot| slot.clone())
    }

    fn resumed(&self) -> bool {
        self.resumed.load(Ordering::SeqCst)
    }

    /// Drop the transcript the CLI wrote for this conversation in the owned config dir.
    async fn forget(&self, resume_token: &str) -> Result<(), HarnessError> {
        let projects = self.launch.config_dir.join("projects");
        remove_transcript(&projects, resume_token);
        Ok(())
    }
}

fn remove_transcript(dir: &Path, token: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_transcript(&path, token);
        } else if path.file_stem().and_then(|stem| stem.to_str()) == Some(token) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wiring() -> HarnessWiring {
        HarnessWiring {
            proxy_url: "http://127.0.0.1:9/proxy/tok".into(),
            mcp_url: None,
            mcp_command: Some(vec!["emberloop".into(), "mcp".into()]),
            env: Vec::new(),
            model: Some("claude/opus".into()),
            thought_level: Some("high".into()),
            resume_token: None,
            workdir: None,
        }
    }

    fn harness(dir: &Path) -> ClaudeHarness {
        ClaudeHarness::new("/bin/true", dir)
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn the_flag_list_is_pinned_and_carries_no_proxy_variables() {
        let dir = tempfile::tempdir().unwrap();
        let launch = harness(dir.path()).launch("s1", &wiring()).unwrap();
        assert_eq!(
            launch.args,
            vec![
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--replay-user-messages",
                "--setting-sources",
                "user",
                "--permission-mode",
                "dontAsk",
                "--permission-prompts",
                "none",
                "--allowedTools",
                "mcp__el",
                "--tools",
                "",
                "--disallowedTools",
                &DENIED_TOOLS.join(","),
                "--settings",
                r#"{"awaySummaryEnabled":false}"#,
                "--model",
                "opus",
                "--effort",
                "high",
                "--strict-mcp-config",
                "--mcp-config",
                launch.args[launch.args.len() - 1].as_str(),
            ]
        );
        let config: serde_json::Value =
            serde_json::from_str(&launch.args[launch.args.len() - 1]).unwrap();
        assert_eq!(config["mcpServers"]["el"]["command"], "emberloop");
        // Our tools are never deferred behind tool search.
        assert_eq!(config["mcpServers"]["el"]["alwaysLoad"], true);
        let env: Vec<&str> = launch.env.iter().map(|(key, _)| key.as_str()).collect();
        assert!(launch.env.contains(&(
            "ANTHROPIC_BASE_URL".to_string(),
            "http://127.0.0.1:9/proxy/tok/claude".to_string(),
        )));
        assert!(!env.iter().any(|key| key.contains("TOKEN")));
        assert!(launch.env.iter().any(|(k, _)| k == "CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn the_cli_env_is_an_allow_list_that_bypasses_any_proxy_for_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let launch = harness(dir.path()).launch("s1", &wiring()).unwrap();
        let mut keys: Vec<&str> = launch.env.iter().map(|(key, _)| key.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "ANTHROPIC_BASE_URL",
                "CLAUDE_CODE_DISABLE_AGENT_VIEW",
                "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
                "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
                "CLAUDE_CODE_DISABLE_BUNDLED_SKILLS",
                "CLAUDE_CODE_DISABLE_CLAUDE_MDS",
                "CLAUDE_CODE_DISABLE_CRON",
                "CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY",
                "CLAUDE_CODE_DISABLE_FILE_CHECKPOINTING",
                "CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS",
                "CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE",
                "CLAUDE_CODE_DISABLE_WORKFLOWS",
                "CLAUDE_CODE_ENABLE_AWAY_SUMMARY",
                "CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION",
                "CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS",
                "CLAUDE_CODE_SEND_FEEDBACK",
                "CLAUDE_CONFIG_DIR",
                "DISABLE_AUTO_COMPACT",
                "DISABLE_COMPACT",
                "DISABLE_ERROR_REPORTING",
                "DISABLE_FEEDBACK_COMMAND",
                "DISABLE_GROWTHBOOK",
                "DISABLE_TELEMETRY",
                "ENABLE_CLAUDEAI_MCP_SERVERS",
                "ENABLE_TOOL_SEARCH",
                "HOME",
                "NO_PROXY",
                "PATH",
                "no_proxy"
            ]
        );
        for key in ["NO_PROXY", "no_proxy"] {
            let value = launch.env.iter().find(|(name, _)| name == key);
            assert_eq!(value.map(|(_, v)| v.as_str()), Some("127.0.0.1,localhost"));
        }
    }

    #[test]
    fn the_system_text_comes_from_the_proxy_never_the_command_line() {
        let dir = tempfile::tempdir().unwrap();
        let launch = harness(dir.path()).launch("s1", &wiring()).unwrap();
        assert!(!launch.args.iter().any(|arg| arg.starts_with("--system")));
    }

    #[test]
    fn no_effort_flag_for_an_unknown_level() {
        let dir = tempfile::tempdir().unwrap();
        let mut wiring = wiring();
        wiring.thought_level = Some("off".into());
        let launch = harness(dir.path()).launch("s1", &wiring).unwrap();
        assert!(!launch.args.iter().any(|arg| arg == "--effort"));
    }

    #[test]
    fn the_cli_auto_compaction_is_disabled_for_every_session() {
        let dir = tempfile::tempdir().unwrap();
        let launch = harness(dir.path()).launch("s1", &wiring()).unwrap();
        for key in ["DISABLE_AUTO_COMPACT", "DISABLE_COMPACT"] {
            assert!(
                launch.env.contains(&(key.to_string(), "1".to_string())),
                "{key} is not set to 1"
            );
        }
    }

    #[test]
    fn every_disabling_variable_reaches_the_child_env_with_its_value() {
        let dir = tempfile::tempdir().unwrap();
        let launch = harness(dir.path()).launch("s1", &wiring()).unwrap();
        for (key, value) in CLI_DISABLING_ENV {
            assert!(
                launch.env.contains(&(key.to_string(), value.to_string())),
                "{key} is not set to {value} in the child env"
            );
        }
    }

    #[test]
    fn the_inline_settings_turn_the_away_summary_off_for_user_only_sources() {
        let dir = tempfile::tempdir().unwrap();
        let launch = harness(dir.path()).launch("s1", &wiring()).unwrap();
        let position = launch
            .args
            .iter()
            .position(|arg| arg == "--settings")
            .unwrap();
        let settings: serde_json::Value = serde_json::from_str(&launch.args[position + 1]).unwrap();
        assert_eq!(settings, json!({"awaySummaryEnabled": false}));
        let sources = launch
            .args
            .iter()
            .position(|arg| arg == "--setting-sources")
            .unwrap();
        assert_eq!(launch.args[sources + 1], "user");
    }

    #[test]
    fn the_deny_list_covers_every_tool_of_the_recorded_init() {
        let init = fixture("init-default-tools.json");
        let outputs = parser::parse_line(&init);
        let Some(Output::Init(init)) = outputs.first() else {
            panic!("fixture is not an init line");
        };
        assert!(!init.tools.is_empty());
        for tool in &init.tools {
            assert!(
                DENIED_TOOLS.contains(&tool.as_str()),
                "{tool} is offered by the CLI but not denied"
            );
        }
    }

    #[test]
    fn fixtures_map_to_the_exact_events() {
        let events: Vec<HarnessEvent> = fixture("turn.jsonl")
            .lines()
            .flat_map(parser::parse_line)
            .filter_map(|output| match output {
                Output::Event(event) => Some(event),
                Output::Init(_) | Output::Consumed => None,
            })
            .collect();
        assert_eq!(
            events,
            vec![
                HarnessEvent::AssistantChunk("Hello".into()),
                HarnessEvent::AssistantChunk(" there".into()),
                HarnessEvent::ThoughtChunk("checking".into()),
                HarnessEvent::ToolStarted {
                    id: "toolu_1".into(),
                    name: "mcp__el__bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                },
                HarnessEvent::ToolFinished {
                    id: "toolu_1".into(),
                    output: serde_json::json!("a.txt"),
                    status: None,
                },
                HarnessEvent::TurnFinished {
                    stop_reason: Some("end_turn".into()),
                    usage: Some(serde_json::json!({"input_tokens": 4, "output_tokens": 9})),
                },
            ]
        );
    }

    #[test]
    fn a_replayed_user_message_is_the_cli_taking_it() {
        let taken = r#"{"type":"user","isReplay":true,"message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(parser::parse_line(taken), vec![Output::Consumed]);
    }

    #[test]
    fn an_auth_failure_and_an_unknown_line_are_mapped() {
        let outputs: Vec<Output> = fixture("auth-failed.jsonl")
            .lines()
            .flat_map(parser::parse_line)
            .collect();
        assert!(outputs.iter().any(|output| matches!(
            output,
            Output::Event(HarnessEvent::Error(text)) if text.contains("Not logged in")
        )));
        assert!(outputs
            .iter()
            .any(|output| matches!(output, Output::Event(HarnessEvent::TurnFinished { .. }))));
        assert!(parser::parse_line("{\"type\":\"loud_new_event\"}").is_empty());
        assert!(parser::parse_line("not json").is_empty());
    }

    #[test]
    fn init_refuses_a_failed_mcp_server() {
        let line = fixture("init-mcp-failed.json");
        let Some(Output::Init(init)) = parser::parse_line(&line).into_iter().next() else {
            panic!("fixture is not an init line");
        };
        assert!(!init.connected("el"));
        let Some(Output::Init(ok)) = parser::parse_line(&fixture("init-connected.json"))
            .into_iter()
            .next()
        else {
            panic!("fixture is not an init line");
        };
        assert!(ok.connected("el"));
    }

    #[test]
    fn a_user_message_is_one_json_line_with_its_images() {
        let line = user_message(
            "hi",
            &[PromptImage {
                mime_type: "image/png".into(),
                data_base64: "AAAA".into(),
            }],
        );
        assert!(line.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["type"], "user");
        assert_eq!(value["message"]["content"][0]["text"], "hi");
        assert_eq!(value["message"]["content"][1]["type"], "image");
    }

    fn creds(token: &str, expires_at: u64) -> String {
        json!({"claudeAiOauth": {"accessToken": token, "refreshToken": "r", "expiresAt": expires_at}})
            .to_string()
    }

    /// An Account home and a Session home, both existing, under one tempdir.
    fn homes() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let account = dir.path().join("default");
        let session = dir.path().join("s1");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::create_dir_all(&session).unwrap();
        (dir, account, session)
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn a_missing_session_credential_is_seeded_from_the_account() {
        let (_dir, account, session) = homes();
        std::fs::write(account.join(CREDENTIALS), creds("acc", 1000)).unwrap();
        sync_account(&account, &session);
        assert_eq!(read(&session.join(CREDENTIALS)), creds("acc", 1000));
    }

    #[test]
    fn a_newer_account_credential_replaces_the_session_copy() {
        let (_dir, account, session) = homes();
        std::fs::write(account.join(CREDENTIALS), creds("acc", 2000)).unwrap();
        std::fs::write(session.join(CREDENTIALS), creds("ses", 1000)).unwrap();
        sync_account(&account, &session);
        assert_eq!(read(&session.join(CREDENTIALS)), creds("acc", 2000));
        assert_eq!(read(&account.join(CREDENTIALS)), creds("acc", 2000));
    }

    #[test]
    fn a_newer_session_credential_is_pushed_back_to_the_account() {
        let (_dir, account, session) = homes();
        std::fs::write(account.join(CREDENTIALS), creds("acc", 1000)).unwrap();
        std::fs::write(session.join(CREDENTIALS), creds("ses", 2000)).unwrap();
        sync_account(&account, &session);
        assert_eq!(read(&account.join(CREDENTIALS)), creds("ses", 2000));
    }

    #[test]
    fn a_session_credential_creates_a_missing_account_home() {
        let dir = tempfile::tempdir().unwrap();
        let account = dir.path().join("default");
        let session = dir.path().join("s1");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(session.join(CREDENTIALS), creds("ses", 2000)).unwrap();
        sync_account(&account, &session);
        assert_eq!(read(&account.join(CREDENTIALS)), creds("ses", 2000));
    }

    #[test]
    fn equal_expiry_leaves_both_files_alone() {
        let (_dir, account, session) = homes();
        std::fs::write(account.join(CREDENTIALS), creds("acc", 1000)).unwrap();
        std::fs::write(session.join(CREDENTIALS), creds("ses", 1000)).unwrap();
        sync_account(&account, &session);
        assert_eq!(read(&account.join(CREDENTIALS)), creds("acc", 1000));
        assert_eq!(read(&session.join(CREDENTIALS)), creds("ses", 1000));
    }

    #[test]
    fn an_unreadable_session_file_never_overwrites_the_account() {
        let (_dir, account, session) = homes();
        std::fs::write(account.join(CREDENTIALS), creds("acc", 1000)).unwrap();
        std::fs::write(session.join(CREDENTIALS), "not json {").unwrap();
        sync_account(&account, &session);
        assert_eq!(read(&account.join(CREDENTIALS)), creds("acc", 1000));
        assert_eq!(read(&session.join(CREDENTIALS)), creds("acc", 1000));
    }

    #[test]
    fn an_unreadable_account_file_is_replaced_by_the_session_copy() {
        let (_dir, account, session) = homes();
        std::fs::write(account.join(CREDENTIALS), "{}").unwrap();
        std::fs::write(session.join(CREDENTIALS), creds("ses", 1000)).unwrap();
        sync_account(&account, &session);
        assert_eq!(read(&account.join(CREDENTIALS)), creds("ses", 1000));
    }

    #[test]
    fn a_credential_without_an_expiry_is_no_usable_credential() {
        let (_dir, account, session) = homes();
        std::fs::write(
            account.join(CREDENTIALS),
            r#"{"claudeAiOauth":{"accessToken":"a"}}"#,
        )
        .unwrap();
        std::fs::write(session.join(CREDENTIALS), creds("ses", 1000)).unwrap();
        sync_account(&account, &session);
        assert_eq!(read(&account.join(CREDENTIALS)), creds("ses", 1000));
    }

    #[test]
    fn no_credential_anywhere_is_not_an_error() {
        let (_dir, account, session) = homes();
        sync_account(&account, &session);
        assert!(!account.join(CREDENTIALS).exists());
        assert!(!session.join(CREDENTIALS).exists());
    }

    #[test]
    fn a_synced_credential_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, account, session) = homes();
        std::fs::write(account.join(CREDENTIALS), creds("acc", 1000)).unwrap();
        std::fs::write(session.join(CREDENTIALS), creds("ses", 2000)).unwrap();
        sync_account(&account, &session);
        let mode = std::fs::metadata(account.join(CREDENTIALS))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let leftovers = std::fs::read_dir(&account)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0, "no temp file survives the rename");
    }

    #[test]
    fn claude_json_is_only_seeded_when_the_session_has_none() {
        let (_dir, account, session) = homes();
        std::fs::write(account.join(CLAUDE_JSON), "theirs").unwrap();
        sync_account(&account, &session);
        assert_eq!(read(&session.join(CLAUDE_JSON)), "theirs");
        std::fs::write(session.join(CLAUDE_JSON), "mine").unwrap();
        std::fs::write(account.join(CLAUDE_JSON), "newer").unwrap();
        sync_account(&account, &session);
        assert_eq!(read(&session.join(CLAUDE_JSON)), "mine");
    }
}
