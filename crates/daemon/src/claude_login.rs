//! Claude subscription login (spec P2-2). The daemon spawns `claude setup-token` under a pty in
//! the Account's own `CLAUDE_CONFIG_DIR`, hands the sign-in URL to the panel, and writes the
//! pasted code back to the CLI. The CLI prints a one-year token that never refreshes; the daemon
//! stores it in `<home>/oauth-token` and Sessions pass it as `CLAUDE_CODE_OAUTH_TOKEN`. A rotating
//! refresh token copied into many homes gets revoked by whichever home refreshes second.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use api::providers::{AccountView, LoginError, LoginView, ProviderView};
use harness_claude::ACCOUNT_HOME;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;

const LOGIN_LIFETIME_SECS: u64 = 10 * 60;
const OUTPUT_MAX: usize = 8 * 1024;
const RAW_MAX: usize = 64 * 1024;
const TOKEN_FILE: &str = "oauth-token";
const SIGN_IN: &str = "https://claude.com/cai/oauth/authorize";

struct Active {
    view: Mutex<LoginView>,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
}

/// The `claude setup-token` flow and the home a Session reads its token from.
pub struct ClaudeLogin {
    bin: PathBuf,
    homes: PathBuf,
    active: Mutex<Option<Arc<Active>>>,
}

impl Clone for ClaudeLogin {
    fn clone(&self) -> Self {
        Self::new(self.bin.clone(), self.homes.clone())
    }
}

impl ClaudeLogin {
    pub fn new(bin: impl Into<PathBuf>, homes: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            homes: homes.into(),
            active: Mutex::new(None),
        }
    }

    /// The Account home: Sessions copy this credential into their owned home.
    pub fn home(&self) -> PathBuf {
        self.homes.join(ACCOUNT_HOME)
    }

    pub fn credentials(&self) -> PathBuf {
        self.home().join(".credentials.json")
    }

    pub fn signed_in(&self) -> bool {
        std::fs::read_to_string(self.home().join(TOKEN_FILE)).is_ok_and(|t| !t.trim().is_empty())
            || access_token(&self.credentials()).is_some()
    }

    pub async fn provider(&self) -> ProviderView {
        let signed_in = self.signed_in();
        let email = account_email(&self.home().join(".claude.json"));
        let tier = subscription(&self.credentials()).unwrap_or_default();
        ProviderView {
            id: "claude".into(),
            name: "Claude".into(),
            signed_in,
            auth: "login".into(),
            accounts: signed_in
                .then(|| AccountView {
                    id: "claude".into(),
                    label: if email.is_empty() {
                        "Claude subscription".into()
                    } else {
                        email
                    },
                    tier,
                    signed_in,
                    last_login: 0,
                })
                .into_iter()
                .collect(),
        }
    }

    /// A login still running is adopted, not refused: a reloaded panel, or a second click, can
    /// finish the sign-in that is already open. A login past its lifetime is dropped and replaced,
    /// so a CLI left waiting for a code cannot block every later attempt.
    pub async fn start(&self, _label: &str) -> Result<LoginView, LoginError> {
        if let Some(login) = self.active.lock().await.as_ref().map(Arc::clone) {
            let view = login.view.lock().await.clone();
            if view.state == "running" && !expired(&view) {
                return Ok(view);
            }
        }
        std::fs::create_dir_all(self.home()).map_err(LoginError::server)?;
        // `setup-token` draws a terminal UI and prints nothing without a pty; `script` supplies one.
        let mut child = Command::new("script")
            .args([
                "-qfc",
                "stty cols 500; exec \"$CLAUDE_BIN\" setup-token",
                "/dev/null",
            ])
            .env("CLAUDE_BIN", &self.bin)
            .env("CLAUDE_CONFIG_DIR", self.home())
            .env("HOME", self.home())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                LoginError::unavailable(format!("the claude CLI cannot start: {error}"))
            })?;
        let stdin = child.stdin.take();
        let view = LoginView {
            id: format!("claudelogin_{}", now_ms()),
            state: "running".into(),
            expires_at: Some(now_ms() + LOGIN_LIFETIME_SECS * 1000),
            ..LoginView::default()
        };
        let login = Arc::new(Active {
            view: Mutex::new(view.clone()),
            stdin: Mutex::new(stdin),
            child: Mutex::new(Some(child)),
        });
        *self.active.lock().await = Some(Arc::clone(&login));
        tokio::spawn(supervise(login, self.home()));
        Ok(view)
    }

    pub async fn status(&self, id: &str) -> Option<LoginView> {
        let login = self.active.lock().await.as_ref().map(Arc::clone)?;
        let view = login.view.lock().await.clone();
        (view.id == id).then_some(view)
    }

    pub async fn cancel(&self, id: &str) -> Result<(), LoginError> {
        let login = self.active.lock().await.as_ref().map(Arc::clone);
        let Some(login) = login else {
            return Err(LoginError::not_found("the Claude login is not running"));
        };
        let mut view = login.view.lock().await;
        if view.id != id {
            return Err(LoginError::not_found("the Claude login is not running"));
        }
        if view.state == "running" {
            if let Some(child) = login.child.lock().await.as_mut() {
                let _ = child.start_kill();
            }
            view.state = "canceled".into();
        }
        Ok(())
    }

    /// Writes the code the sign-in page showed; the CLI finishes the exchange.
    pub async fn complete(&self, id: &str, code: &str) -> Result<LoginView, LoginError> {
        let login = self.active.lock().await.as_ref().map(Arc::clone);
        let Some(login) = login else {
            return Err(LoginError::not_found("the Claude login is not running"));
        };
        {
            let view = login.view.lock().await;
            if view.id != id {
                return Err(LoginError::not_found("the Claude login is not running"));
            }
            if view.state != "running" {
                return Err(LoginError::conflict("the Claude login is not running"));
            }
        }
        {
            let mut stdin = login.stdin.lock().await;
            let stdin = stdin.as_mut().ok_or_else(|| {
                LoginError::conflict("the Claude login is not waiting for a code")
            })?;
            let written = async {
                stdin
                    .write_all(format!("{}\r", code.trim()).as_bytes())
                    .await?;
                stdin.flush().await
            };
            written.await.map_err(|error| {
                LoginError::server(format!("the claude CLI refused the code: {error}"))
            })?;
        }
        self.status(id)
            .await
            .ok_or_else(|| LoginError::not_found("the Claude login is not running"))
    }

    pub async fn sign_out(&self) -> Result<(), LoginError> {
        for path in [self.credentials(), self.home().join(TOKEN_FILE)] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(LoginError::server(error)),
            }
        }
        Ok(())
    }
}

/// Reads the pty until the CLI exits or prints the long-lived token, then reports the outcome.
/// The token is written to `<home>/oauth-token` and never shown in the output.
async fn supervise(login: Arc<Active>, home: PathBuf) {
    let (stdout, stderr) = match login.child.lock().await.as_mut() {
        Some(child) => (child.stdout.take(), child.stderr.take()),
        None => (None, None),
    };
    if let Some(stderr) = stderr {
        tokio::spawn(pump_stderr(Arc::clone(&login), stderr));
    }
    let token = match stdout {
        Some(stdout) => read_output(&login, stdout).await,
        None => None,
    };
    {
        let mut child = login.child.lock().await;
        match (token.is_some(), child.as_mut()) {
            (true, Some(child)) => {
                let _ = child.start_kill();
            }
            (false, Some(child)) => {
                let _ = child.wait().await;
            }
            _ => {}
        }
    }
    let mut view = login.view.lock().await;
    if view.state != "running" {
        return;
    }
    let saved = match token {
        Some(token) => {
            save_token(&home, &token).map_err(|e| format!("the token could not be saved: {e}"))
        }
        None if !view.error.is_empty() => Err(view.error.clone()),
        None => Err("The claude CLI did not print a token. Try again.".to_string()),
    };
    (view.state, view.error) = match saved {
        Ok(()) => ("succeeded".into(), String::new()),
        Err(error) => ("failed".into(), error),
    };
}

async fn pump_stderr(login: Arc<Active>, mut stderr: tokio::process::ChildStderr) {
    let mut chunk = [0u8; 4096];
    while let Ok(n @ 1..) = stderr.read(&mut chunk).await {
        push(&login, &String::from_utf8_lossy(&chunk[..n])).await;
    }
}

/// Collects the sign-in URL into the view; returns the token once the pty prints it.
async fn read_output(login: &Active, mut stdout: tokio::process::ChildStdout) -> Option<String> {
    let (mut raw, mut chunk) = (Vec::new(), [0u8; 4096]);
    while let Ok(n @ 1..) = stdout.read(&mut chunk).await {
        raw.extend_from_slice(&chunk[..n]);
        if raw.len() > RAW_MAX {
            raw.drain(..raw.len() - RAW_MAX);
        }
        let text = String::from_utf8_lossy(&raw).into_owned();
        let mut view = login.view.lock().await;
        if let (true, Some(at)) = (view.url.is_empty(), text.find(SIGN_IN)) {
            let rest = &text[at..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c.is_control())
                .unwrap_or(rest.len());
            view.url = rest[..end].to_string();
        }
        drop(view);
        let clean = strip_ansi(&text);
        if let Some(found) = find_token(&clean) {
            return Some(found);
        }
        // A rejected code leaves the CLI at "Press Enter to retry"; end the login so the next click starts a fresh one.
        if let Some(at) = clean.find("OAuth error:") {
            let message = clean[at..].split("Press Enter").next().unwrap_or_default();
            login.view.lock().await.error =
                message.split_whitespace().collect::<Vec<_>>().join(" ");
            if let Some(child) = login.child.lock().await.as_mut() {
                let _ = child.start_kill();
            }
            return None;
        }
    }
    None
}

/// Terminal escape sequences become spaces, so text they split stays split.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        out.push(' ');
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A complete token is followed by a non-token character (the pty ends the line with a newline).
fn find_token(text: &str) -> Option<String> {
    let rest = &text[text.find("sk-ant-oat")?..];
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))?;
    Some(rest[..end].to_string())
}

fn save_token(home: &Path, token: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let temp = home.join("oauth-token.tmp");
    std::fs::write(&temp, token)?;
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(temp, home.join(TOKEN_FILE))
}

/// A login past `expires_at` no longer blocks a new one.
fn expired(view: &LoginView) -> bool {
    view.expires_at.is_some_and(|at| at <= now_ms())
}

async fn push(login: &Active, text: &str) {
    let mut view = login.view.lock().await;
    view.output.push_str(&strip_ansi(text));
    if view.output.len() > OUTPUT_MAX {
        let mut cut = view.output.len() - OUTPUT_MAX;
        while !view.output.is_char_boundary(cut) {
            cut += 1;
        }
        view.output = view.output[cut..].to_string();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

fn read_json(path: &Path) -> Value {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

fn access_token(path: &Path) -> Option<String> {
    oauth(path)
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

fn subscription(path: &Path) -> Option<String> {
    oauth(path)
        .get("subscriptionType")
        .and_then(Value::as_str)
        .filter(|tier| !tier.is_empty())
        .map(str::to_owned)
}

fn oauth(path: &Path) -> Value {
    let data = read_json(path);
    let oauth = data.get("claudeAiOauth").cloned().unwrap_or(data);
    if oauth.is_object() {
        oauth
    } else {
        Value::Null
    }
}

fn account_email(path: &Path) -> String {
    read_json(path)
        .get("oauthAccount")
        .and_then(|account| account.get("emailAddress"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
