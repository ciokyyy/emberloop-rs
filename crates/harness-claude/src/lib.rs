#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! The real `claude` binary over newline-delimited JSON on its stdin and stdout:
//! no ACP adapter, no Node, no SDK crate between the daemon and `claude`.
pub mod parser;

use async_trait::async_trait;
use harness::{EventStream, Harness, HarnessError, HarnessSession, HarnessWiring, PromptImage};
use parser::Output;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{timeout, Duration};

/// A CLI that never reaches `system/init` is a broken launch, not a slow one.
const INIT_TIMEOUT: Duration = Duration::from_secs(30);
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
        seed_account(&self.homes.join(ACCOUNT_HOME), &config_dir);
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
            "--setting-sources",
            "user",
            "--permission-mode",
            "bypassPermissions",
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
        if let Some(prompt) = &wiring.system_prompt {
            args.push("--system-prompt".into());
            args.push(prompt.clone());
        }
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
        for (key, value) in &wiring.env {
            env.push((key.clone(), value.clone()));
        }
        Ok(Launch {
            program: self.bin.clone(),
            args,
            env,
            cwd,
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
}

/// Our MCP server alone: `el` over stdio, or the same name over HTTP.
/// Copies the Account credential into a Session home that has none yet, so a
/// logged-in Account serves every Session without sharing their transcripts.
fn seed_account(account_home: &Path, config_dir: &Path) {
    for name in [".credentials.json", ".claude.json"] {
        let target = config_dir.join(name);
        if !target.exists() {
            let _ = std::fs::copy(account_home.join(name), &target);
        }
    }
}

fn mcp_config(wiring: &HarnessWiring) -> Option<String> {
    let server = match (&wiring.mcp_url, &wiring.mcp_command) {
        (Some(url), _) => json!({"type": "http", "url": url}),
        (None, Some(command)) if !command.is_empty() => {
            json!({"command": command[0], "args": &command[1..]})
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
}

pub struct ClaudeSession {
    launch: Launch,
    state: Mutex<Process>,
    /// The CLI's own id for this conversation, filled by `system/init`.
    session_id: Arc<StdMutex<Option<String>>>,
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
                resume,
            }),
            session_id: Arc::new(StdMutex::new(None)),
            resumed: AtomicBool::new(false),
            tx,
            rx: StdMutex::new(Some(rx)),
        }
    }

    /// Start the CLI if it is not alive and wait for `system/init`; refuse a Session
    /// whose `el` MCP server did not connect.
    async fn ensure(&self) -> Result<(), HarnessError> {
        let mut state = self.state.lock().await;
        if let Some(child) = state.child.as_mut() {
            if child.try_wait()?.is_none() {
                return Ok(());
            }
            state.child = None;
            state.stdin = None;
        }
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
            .stderr(Stdio::null())
            // `env_clear` keeps `ANTHROPIC_BASE_URL` and any inherited token out.
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
        let (init_tx, init_rx) = oneshot::channel();
        tokio::spawn(read_stream(
            stdout,
            Arc::clone(&self.session_id),
            self.tx.clone(),
            init_tx,
        ));
        match timeout(INIT_TIMEOUT, init_rx).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => return Err(HarnessError::Protocol(error)),
            Ok(Err(_)) => {
                return Err(HarnessError::Process("claude exited before init".into()));
            }
            Err(_) => return Err(HarnessError::Process("claude did not init".into())),
        }
        self.resumed.store(token.is_some(), Ordering::SeqCst);
        state.child = Some(child);
        state.stdin = Some(stdin);
        Ok(())
    }

    async fn write(&self, line: String) -> Result<(), HarnessError> {
        self.ensure().await?;
        let mut state = self.state.lock().await;
        let stdin = state
            .stdin
            .as_mut()
            .ok_or_else(|| HarnessError::Process("claude: stdin closed".into()))?;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }
}

/// Every line the CLI prints; `system/init` answers the spawn, the rest becomes events.
async fn read_stream(
    stdout: tokio::process::ChildStdout,
    session_id: Arc<StdMutex<Option<String>>>,
    tx: mpsc::UnboundedSender<harness::HarnessEvent>,
    init: oneshot::Sender<Result<(), String>>,
) {
    let mut init = Some(init);
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        for output in parser::parse_line(&line) {
            match output {
                Output::Init(found) => accept_init(found, &mut init, &session_id),
                Output::Event(event) => {
                    let _ = tx.send(event);
                }
            }
        }
    }
    if let Some(tx) = init.take() {
        let _ = tx.send(Err("claude exited before init".into()));
    }
    let _ = tx.send(harness::HarnessEvent::Error("claude exited".into()));
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
        let mut state = self.state.lock().await;
        if let Some(pid) = state.child.as_mut().and_then(|child| child.id()) {
            // SAFETY: `pid` is our live child; SIGINT only ends its current turn.
            unsafe {
                libc::kill(pid as i32, libc::SIGINT);
            }
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
    async fn wake(&self, text: &str) -> Result<(), HarnessError> {
        self.write(user_message(text, &[])).await
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
    use harness::HarnessEvent;

    fn wiring() -> HarnessWiring {
        HarnessWiring {
            proxy_url: "http://127.0.0.1:9/proxy/tok".into(),
            mcp_url: None,
            mcp_command: Some(vec!["emberloop".into(), "mcp".into()]),
            env: Vec::new(),
            system_prompt: None,
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
                "--setting-sources",
                "user",
                "--permission-mode",
                "bypassPermissions",
                "--permission-prompts",
                "none",
                "--allowedTools",
                "mcp__el",
                "--tools",
                "",
                "--disallowedTools",
                &DENIED_TOOLS.join(","),
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
        let env: Vec<&str> = launch.env.iter().map(|(key, _)| key.as_str()).collect();
        assert!(!env.contains(&"ANTHROPIC_BASE_URL"));
        assert!(!env.iter().any(|key| key.contains("TOKEN")));
        assert!(launch.env.iter().any(|(k, _)| k == "CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn a_system_prompt_is_passed_through_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let mut wiring = wiring();
        wiring.system_prompt = Some("the one policy text".into());
        let launch = harness(dir.path()).launch("s1", &wiring).unwrap();
        let at = launch.args.iter().position(|arg| arg == "--system-prompt");
        let at = at.unwrap();
        assert_eq!(launch.args[at + 1], "the one policy text");
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
                Output::Init(_) => None,
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
}
