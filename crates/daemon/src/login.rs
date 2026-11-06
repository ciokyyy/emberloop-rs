//! Providers and the agy login (spec decision P). The daemon spawns the real
//! agy ACP server for the Account's own `GEMINI_HOME`, sends `authenticate`
//! with `oauth-personal`, hands the sign-in URL from stderr to the panel, and
//! completes the exchange by GETting the pasted loopback address. No token ever
//! passes through the daemon: the server owns it, and the daemon only reads
//! the tier the server reports.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::codex::CodexLogin;
use api::providers::{AccountView, LoginError, LoginView, ProviderLogin, ProviderView};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

const ACCOUNT_ID: &str = "default";
const LOGIN_TIMEOUT_SECS: u64 = 10 * 60;
const OUTPUT_MAX: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AccountsFile {
    accounts: Vec<StoredAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredAccount {
    id: String,
    label: String,
    tier: String,
    last_login: u64,
}

struct LoginConn {
    stdin: Mutex<ChildStdin>,
    waiters: Mutex<HashMap<i64, oneshot::Sender<Result<serde_json::Value, String>>>>,
    next: AtomicI64,
}

impl LoginConn {
    async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel();
        self.waiters.lock().await.insert(id, sender);
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut stdin = self.stdin.lock().await;
        let write = async {
            stdin
                .write_all(format!("{payload}\n").as_bytes())
                .await
                .map_err(|error| format!("agy login: write {method}: {error}"))?;
            stdin
                .flush()
                .await
                .map_err(|error| format!("agy login: write {method}: {error}"))
        };
        write.await?;
        receiver
            .await
            .map_err(|_| "agy login: the Antigravity server closed".to_string())?
    }

    async fn read_stdout(self: Arc<Self>, stdout: tokio::process::ChildStdout) {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(id) = message.get("id").and_then(|id| id.as_i64()) else {
                continue;
            };
            let Some(sender) = self.waiters.lock().await.remove(&id) else {
                continue;
            };
            let answer = match message.get("error") {
                Some(error) => Err(format!(
                    "agy login: {}",
                    error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("the server answered an error")
                )),
                None => Ok(message
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)),
            };
            let _ = sender.send(answer);
        }
        for (_, sender) in self.waiters.lock().await.drain() {
            let _ = sender.send(Err("agy login: the Antigravity server closed".to_string()));
        }
    }
}

struct ActiveLogin {
    view: Mutex<LoginView>,
    label: String,
    conn: Arc<LoginConn>,
    child: Mutex<Option<Child>>,
}

pub struct AgyLogin {
    accounts_dir: PathBuf,
    bin: PathBuf,
    keys_file: Option<PathBuf>,
    active: Mutex<Option<Arc<ActiveLogin>>>,
    http: reqwest::Client,
    codex: CodexLogin,
}

impl AgyLogin {
    pub fn new(accounts_dir: impl Into<PathBuf>, bin: impl Into<PathBuf>) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            accounts_dir: accounts_dir.into(),
            bin: bin.into(),
            keys_file: None,
            active: Mutex::new(None),
            http,
            codex: CodexLogin::new(PathBuf::new()),
        }
    }

    pub fn with_keys(mut self, keys_file: impl Into<PathBuf>) -> Self {
        self.keys_file = Some(keys_file.into());
        self
    }

    pub fn with_pi_auth(mut self, auth_file: impl Into<PathBuf>) -> Self {
        self.codex = CodexLogin::new(auth_file);
        self
    }

    fn home(&self) -> PathBuf {
        self.accounts_dir.join(ACCOUNT_ID)
    }

    fn accounts_file(&self) -> PathBuf {
        self.accounts_dir.join("providers.json")
    }

    fn read_accounts(&self) -> AccountsFile {
        std::fs::read_to_string(self.accounts_file())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write_accounts(&self, accounts: &AccountsFile) -> Result<(), LoginError> {
        let text = serde_json::to_string_pretty(accounts).map_err(LoginError::server)?;
        std::fs::write(self.accounts_file(), text).map_err(LoginError::server)
    }

    async fn fail(&self, login: &ActiveLogin, message: &str) {
        {
            let mut view = login.view.lock().await;
            if view.state != "canceled" {
                view.state = "failed".to_string();
                view.error = message.to_string();
            }
        }
        self.kill(login).await;
        self.remove_account().await;
    }

    async fn kill(&self, login: &ActiveLogin) {
        if let Some(mut child) = login.child.lock().await.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    async fn remove_account(&self) {
        let _ = std::fs::remove_dir_all(self.home());
        let mut accounts = self.read_accounts();
        accounts.accounts.retain(|account| account.id != ACCOUNT_ID);
        let _ = self.write_accounts(&accounts);
        let _ = std::fs::remove_file(self.accounts_file());
    }

    async fn store_account(&self, label: &str, tier: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|time| time.as_secs())
            .unwrap_or_default();
        let accounts = AccountsFile {
            accounts: vec![StoredAccount {
                id: ACCOUNT_ID.to_string(),
                label: label.to_string(),
                tier: tier.to_string(),
                last_login: now,
            }],
        };
        let _ = self.write_accounts(&accounts);
    }

    fn runner(&self) -> AgyLogin {
        AgyLogin {
            accounts_dir: self.accounts_dir.clone(),
            bin: self.bin.clone(),
            keys_file: self.keys_file.clone(),
            active: Mutex::new(None),
            http: self.http.clone(),
            codex: CodexLogin::new(self.codex.auth_path()),
        }
    }

    async fn run(self: Arc<Self>, login: Arc<ActiveLogin>) {
        let stderr_login = Arc::clone(&login);
        let stderr = login
            .child
            .lock()
            .await
            .as_mut()
            .and_then(|child| child.stderr.take());
        if let Some(mut stderr) = stderr {
            tokio::spawn(async move {
                let mut lines = BufReader::new(&mut stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut view = stderr_login.view.lock().await;
                    view.output.push_str(&line);
                    view.output.push('\n');
                    if view.output.len() > OUTPUT_MAX {
                        view.output = view.output[view.output.len() - OUTPUT_MAX..].to_string();
                    }
                    if view.url.is_empty() {
                        if let Some(url) = sign_in_url(&view.output) {
                            view.url = url;
                        }
                    }
                }
            });
        }

        let initialized = login
            .conn
            .call(
                "initialize",
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": {
                        "fs": {"readTextFile": false, "writeTextFile": false},
                        "terminal": false
                    }
                }),
            )
            .await;
        if let Err(error) = initialized {
            self.fail(&login, &error).await;
            return;
        }
        let authenticated = tokio::time::timeout(
            std::time::Duration::from_secs(LOGIN_TIMEOUT_SECS),
            login.conn.call(
                "authenticate",
                serde_json::json!({"methodId": "oauth-personal"}),
            ),
        )
        .await;
        match authenticated {
            Err(_) => self.fail(&login, "the Antigravity login timed out").await,
            Ok(Err(error)) => self.fail(&login, &error).await,
            Ok(Ok(result)) => {
                let tier = tier_of(&result);
                {
                    let mut view = login.view.lock().await;
                    view.tier = tier.clone();
                    view.state = "succeeded".to_string();
                }
                self.store_account(&login.label, &tier).await;
                self.kill(&login).await;
            }
        }
    }
}

fn sign_in_url(output: &str) -> Option<String> {
    for token in output.split_whitespace() {
        let token = token.trim_matches(|c: char| "\"'<>()[]".contains(c));
        if (token.starts_with("https://") || token.starts_with("http://"))
            && token.len() > "https://".len()
        {
            return Some(token.to_string());
        }
    }
    None
}

fn tier_of(result: &serde_json::Value) -> String {
    result
        .pointer("/_meta/antigravity/tier")
        .and_then(|tier| tier.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[async_trait]
impl ProviderLogin for AgyLogin {
    async fn providers(&self) -> Vec<ProviderView> {
        let accounts: Vec<AccountView> = self
            .read_accounts()
            .accounts
            .into_iter()
            .map(|account| AccountView {
                id: account.id,
                label: account.label,
                tier: account.tier,
                signed_in: self.home().join("antigravity-acp/acp_token.json").exists(),
                last_login: account.last_login,
            })
            .collect();
        let mut list = vec![ProviderView {
            id: "agy".to_string(),
            name: "Antigravity".to_string(),
            signed_in: accounts.iter().any(|account| account.signed_in),
            accounts,
        }];
        list.push(self.codex.provider().await);
        if let Some(keys) = &self.keys_file {
            list.push(opencode_provider(keys));
        }
        list
    }

    async fn start(&self, label: &str) -> Result<LoginView, LoginError> {
        let mut active = self.active.lock().await;
        if let Some(login) = active.as_ref() {
            let running = login.view.lock().await.state == "running";
            if running {
                return Err(LoginError::conflict(
                    "an Antigravity login is already running",
                ));
            }
        }
        if !self.bin.exists() && which(&self.bin).is_none() {
            return Err(LoginError::unavailable(
                "the Antigravity server is not installed",
            ));
        }
        std::fs::create_dir_all(self.home()).map_err(LoginError::server)?;
        let mut command = Command::new(&self.bin);
        command
            .arg("--uid=")
            .env("GEMINI_HOME", self.home())
            .env("BROWSER", "/bin/true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            LoginError::unavailable(format!("start the Antigravity server: {error}"))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LoginError::server("the Antigravity server has no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LoginError::server("the Antigravity server has no stdout"))?;
        let conn = Arc::new(LoginConn {
            stdin: Mutex::new(stdin),
            waiters: Mutex::new(HashMap::new()),
            next: AtomicI64::new(0),
        });
        tokio::spawn(Arc::clone(&conn).read_stdout(stdout));
        let view = LoginView {
            id: format!("agylogin_{ACCOUNT_ID}"),
            state: "running".to_string(),
            account_id: Some(ACCOUNT_ID.to_string()),
            ..LoginView::default()
        };
        let label = match label.trim() {
            "" => "Antigravity".to_string(),
            label => label.to_string(),
        };
        let login = Arc::new(ActiveLogin {
            view: Mutex::new(view.clone()),
            label: label.clone(),
            conn,
            child: Mutex::new(Some(child)),
        });
        *active = Some(Arc::clone(&login));
        drop(active);
        self.store_account(&label, "").await;
        tokio::spawn(Arc::new(self.runner()).run(Arc::clone(&login)));
        Ok(view)
    }

    async fn status(&self, id: &str) -> Option<LoginView> {
        let active = self.active.lock().await;
        let login = active.as_ref()?;
        let view = login.view.lock().await.clone();
        (view.id == id).then_some(view)
    }

    async fn cancel(&self, id: &str) -> Result<(), LoginError> {
        let active = self.active.lock().await;
        let Some(login) = active.as_ref() else {
            return Err(LoginError::not_found(
                "the Antigravity login is not running",
            ));
        };
        if login.view.lock().await.id != id {
            return Err(LoginError::not_found(
                "the Antigravity login is not running",
            ));
        }
        {
            let mut view = login.view.lock().await;
            view.state = "canceled".to_string();
        }
        self.kill(login).await;
        self.remove_account().await;
        Ok(())
    }

    async fn complete(&self, id: &str, address: &str) -> Result<LoginView, LoginError> {
        let target = loopback_url(address)?;
        {
            let active = self.active.lock().await;
            let Some(login) = active.as_ref() else {
                return Err(LoginError::not_found(
                    "the Antigravity login is not running",
                ));
            };
            let view = login.view.lock().await;
            if view.id != id {
                return Err(LoginError::not_found(
                    "the Antigravity login is not running",
                ));
            }
            if view.state != "running" {
                return Err(LoginError::conflict("the Antigravity login is not running"));
            }
        }
        let response = self.http.get(target).send().await.map_err(|error| {
            LoginError::bad_request(format!("the Antigravity server did not answer: {error}"))
        })?;
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(LoginError::bad_request(format!(
                "the Antigravity server answered {}",
                response.status()
            )));
        }
        self.status(id)
            .await
            .ok_or_else(|| LoginError::not_found("the Antigravity login is not running"))
    }

    async fn sign_out(&self, id: &str) -> Result<(), LoginError> {
        if id == "openai-codex" {
            return self.codex.sign_out().await;
        }
        if id == "opencode-go" {
            let keys = self
                .keys_file
                .as_deref()
                .ok_or_else(|| LoginError::not_found("unknown account"))?;
            return clear_key(keys, "opencode-go");
        }
        std::fs::remove_dir_all(self.home()).map_err(LoginError::server)?;
        match std::fs::remove_file(self.accounts_file()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(LoginError::server(error)),
        }
    }

    async fn set_key(&self, provider: &str, key: &str) -> Result<(), LoginError> {
        let keys = self
            .keys_file
            .as_deref()
            .ok_or_else(|| LoginError::unavailable("the daemon has no keys store"))?;
        write_key(keys, provider, key)
    }

    async fn start_codex(&self, label: &str) -> Result<LoginView, LoginError> {
        self.codex.start(label).await
    }

    async fn codex_status(&self, id: &str) -> Option<LoginView> {
        self.codex.status(id).await
    }

    async fn cancel_codex(&self, id: &str) -> Result<(), LoginError> {
        self.codex.cancel(id).await
    }
}

fn opencode_provider(keys: &Path) -> ProviderView {
    let signed_in = has_key(keys, "opencode-go");
    let accounts = if signed_in {
        vec![AccountView {
            id: "opencode-go".to_string(),
            label: "OpenCode Go API key".to_string(),
            tier: "Key".to_string(),
            signed_in: true,
            last_login: 0,
        }]
    } else {
        Vec::new()
    };
    ProviderView {
        id: "opencode-go".to_string(),
        name: "OpenCode Go".to_string(),
        signed_in,
        accounts,
    }
}

fn read_keys_map(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|val| val.as_object().cloned())
        .unwrap_or_default()
}

fn has_key(path: &Path, provider: &str) -> bool {
    read_keys_map(path)
        .get(provider)
        .and_then(|val| val.as_str())
        .is_some_and(|key| !key.is_empty())
}

fn write_key(path: &Path, provider: &str, key: &str) -> Result<(), LoginError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(LoginError::bad_request("paste a non-empty API key"));
    }
    let mut map = read_keys_map(path);
    map.insert(
        provider.to_string(),
        serde_json::Value::String(key.to_string()),
    );
    save_keys_map(path, &map)
}

fn clear_key(path: &Path, provider: &str) -> Result<(), LoginError> {
    let mut map = read_keys_map(path);
    map.remove(provider);
    save_keys_map(path, &map)
}

fn save_keys_map(
    path: &Path,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), LoginError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(LoginError::server)?;
    }
    let text = serde_json::to_string_pretty(map).map_err(LoginError::server)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(LoginError::server)
}

fn which(bin: &Path) -> Option<PathBuf> {
    let name = bin.to_string_lossy();
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(&*name))
        .find(|candidate| candidate.is_file())
}

fn loopback_url(raw: &str) -> Result<reqwest::Url, LoginError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(LoginError::bad_request(
            "paste the address the browser shows after signing in",
        ));
    }
    let url = reqwest::Url::parse(raw)
        .map_err(|_| LoginError::bad_request("the pasted address is not a URL"))?;
    if url.scheme() != "http" {
        return Err(LoginError::bad_request(
            "the pasted address must start with http://127.0.0.1",
        ));
    }
    match url.host_str() {
        Some("127.0.0.1") | Some("localhost") => {}
        _ => {
            return Err(LoginError::bad_request(
                "the pasted address must point at 127.0.0.1",
            ))
        }
    }
    if url.port().is_none() || url.username() != "" || url.password().is_some() {
        return Err(LoginError::bad_request(
            "the pasted address must carry the port the server listens on",
        ));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_url_is_read_from_the_stderr_line() {
        let output = "Open the following link to authenticate the ACP server: https://accounts.example/signin?x=1\n";
        assert_eq!(
            sign_in_url(output).as_deref(),
            Some("https://accounts.example/signin?x=1")
        );
    }

    #[test]
    fn only_loopback_addresses_complete_a_login() {
        assert!(loopback_url("http://127.0.0.1:49152/oauth?code=1").is_ok());
        assert!(loopback_url("http://localhost:49152/").is_ok());
        assert!(loopback_url("https://127.0.0.1:49152/").is_err());
        assert!(loopback_url("http://evil.example:49152/").is_err());
        assert!(loopback_url("http://127.0.0.1/").is_err());
        assert!(loopback_url("").is_err());
    }

    #[test]
    fn the_tier_comes_from_the_authenticate_meta() {
        let result = serde_json::json!({"_meta": {"antigravity": {"tier": " Free "}}});
        assert_eq!(tier_of(&result), "Free");
        assert_eq!(tier_of(&serde_json::json!({})), "");
    }
}
