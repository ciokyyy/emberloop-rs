//! Claude subscription login (spec P2-2). The daemon spawns `claude auth login`
//! in the Account's own `CLAUDE_CONFIG_DIR`, hands the sign-in URL to the panel,
//! and writes the pasted code back to the CLI. The credential never leaves that
//! home; the daemon only reads the tier the CLI stored.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use api::providers::{AccountView, LoginError, LoginView, ProviderView};
use harness_claude::ACCOUNT_HOME;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;

const LOGIN_LIFETIME_SECS: u64 = 10 * 60;
const OUTPUT_MAX: usize = 8 * 1024;
const SIGN_IN: &str = "https://claude.com/cai/oauth/authorize";

struct Active {
    view: Mutex<LoginView>,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
}

/// The `claude auth login` flow and the home a Session copies its credential from.
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
        access_token(&self.credentials()).is_some()
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

    pub async fn start(&self, _label: &str) -> Result<LoginView, LoginError> {
        if let Some(login) = self.active.lock().await.as_ref() {
            if login.view.lock().await.state == "running" {
                return Err(LoginError::conflict("a Claude login is already running"));
            }
        }
        std::fs::create_dir_all(self.home()).map_err(LoginError::server)?;
        let mut child = Command::new(&self.bin)
            .args(["auth", "login"])
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
            // shortcut: the lifetime is shown, not enforced; the next start clears a stale login.
            expires_at: Some(now_ms() + LOGIN_LIFETIME_SECS * 1000),
            ..LoginView::default()
        };
        let login = Arc::new(Active {
            view: Mutex::new(view.clone()),
            stdin: Mutex::new(stdin),
            child: Mutex::new(Some(child)),
        });
        *self.active.lock().await = Some(Arc::clone(&login));
        tokio::spawn(supervise(login, self.credentials()));
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
                    .write_all(format!("{}\n", code.trim()).as_bytes())
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
        let credentials = self.credentials();
        match std::fs::remove_file(&credentials) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(LoginError::server(error)),
        }
    }
}

async fn supervise(login: Arc<Active>, credentials: PathBuf) {
    let (stdout, stderr) = match login.child.lock().await.as_mut() {
        Some(child) => (child.stdout.take(), child.stderr.take()),
        None => (None, None),
    };
    if let Some(stderr) = stderr {
        let reader = Arc::clone(&login);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                push(&reader, &line).await;
            }
        });
    }
    if let Some(stdout) = stdout {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            push(&login, &line).await;
        }
    }
    let status = match login.child.lock().await.take() {
        Some(mut child) => child.wait().await.ok(),
        None => None,
    };
    let success =
        status.is_some_and(|status| status.success()) && access_token(&credentials).is_some();
    let mut view = login.view.lock().await;
    if view.state != "running" {
        return;
    }
    if success {
        view.state = "succeeded".into();
        view.tier = subscription(&credentials).unwrap_or_default();
    } else {
        view.state = "failed".into();
        if view.error.is_empty() {
            view.error = "The claude CLI did not save a credential. Try again.".into();
        }
    }
}

async fn push(login: &Active, line: &str) {
    let mut view = login.view.lock().await;
    view.output.push_str(line);
    view.output.push('\n');
    if view.output.len() > OUTPUT_MAX {
        view.output = view.output[view.output.len() - OUTPUT_MAX..].to_string();
    }
    if view.url.is_empty() {
        if let Some(at) = line.find(SIGN_IN) {
            let rest = &line[at..];
            let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
            view.url = rest[..end].to_string();
        }
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
