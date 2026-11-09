//! ChatGPT device-code login (OpenAI Codex OAuth). The daemon owns the
//! credential: it writes the same `auth.json` shape pi reads, so a Session
//! needs no separate provider implementation.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use api::providers::{AccountView, LoginError, LoginView, ProviderView};
use serde_json::{json, Value};
use tokio::sync::Mutex;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_LIFETIME_SECS: u64 = 15 * 60;
const VERIFY_URI: &str = "https://auth.openai.com/codex/device";
const REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";

#[derive(Debug, Clone)]
pub struct CodexEndpoints {
    pub user_code: String,
    pub device_token: String,
    pub exchange: String,
}

impl Default for CodexEndpoints {
    fn default() -> Self {
        Self {
            user_code: "https://auth.openai.com/api/accounts/deviceauth/usercode".into(),
            device_token: "https://auth.openai.com/api/accounts/deviceauth/token".into(),
            exchange: "https://auth.openai.com/oauth/token".into(),
        }
    }
}

struct Active {
    view: Mutex<LoginView>,
    cancel: AtomicBool,
}

/// ChatGPT device-code login and the `auth.json` entry pi reads.
pub struct CodexLogin {
    auth_file: PathBuf,
    http: reqwest::Client,
    active: Mutex<Option<Arc<Active>>>,
    endpoints: CodexEndpoints,
}

impl CodexLogin {
    pub fn new(auth_file: impl Into<PathBuf>) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            auth_file: auth_file.into(),
            http,
            active: Mutex::new(None),
            endpoints: CodexEndpoints::default(),
        }
    }

    pub fn auth_path(&self) -> &Path {
        &self.auth_file
    }

    /// Test seam: the real flow always uses the OpenAI endpoints.
    pub fn with_endpoints(mut self, endpoints: CodexEndpoints) -> Self {
        self.endpoints = endpoints;
        self
    }

    pub fn signed_in(&self) -> bool {
        read_auth(&self.auth_file)
            .get("openai-codex")
            .and_then(|entry| entry.get("access"))
            .and_then(Value::as_str)
            .is_some_and(|access| !access.is_empty())
    }

    pub async fn provider(&self) -> ProviderView {
        let signed_in = self.signed_in();
        ProviderView {
            id: "openai-codex".into(),
            name: "ChatGPT".into(),
            signed_in,
            auth: "login".into(),
            accounts: signed_in
                .then(|| AccountView {
                    id: "openai-codex".into(),
                    label: "ChatGPT OAuth".into(),
                    tier: "Subscription".into(),
                    signed_in,
                    last_login: 0,
                })
                .into_iter()
                .collect(),
        }
    }

    pub async fn start(&self, _label: &str) -> Result<LoginView, LoginError> {
        if self.auth_file.as_os_str().is_empty() {
            return Err(LoginError::unavailable(
                "the daemon has no pi auth store for ChatGPT",
            ));
        }
        let mut active = self.active.lock().await;
        if let Some(login) = active.as_ref() {
            if login.view.lock().await.state == "running" {
                return Err(LoginError::conflict("a ChatGPT login is already running"));
            }
        }
        let answer = post_json(
            &self.http,
            &self.endpoints.user_code,
            json!({ "client_id": CLIENT_ID }),
        )
        .await?;
        if !answer.status.is_success() {
            return Err(LoginError::server(format!(
                "ChatGPT device code request failed with status {}: {}",
                answer.status,
                answer.text.trim()
            )));
        }
        let device_auth_id = answer.value["device_auth_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let user_code = answer.value["user_code"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let interval = interval_of(&answer.value["interval"])
            .ok_or_else(|| LoginError::server("ChatGPT device code response has no interval"))?;
        if device_auth_id.is_empty() || user_code.is_empty() {
            return Err(LoginError::server(
                "ChatGPT device code response is missing its code",
            ));
        }
        let expires_at = now_ms() + DEVICE_LIFETIME_SECS * 1000;
        let view = LoginView {
            id: format!("codexlogin_{}", now_ms()),
            state: "running".into(),
            user_code: user_code.clone(),
            verification_uri: VERIFY_URI.into(),
            expires_at: Some(expires_at),
            ..LoginView::default()
        };
        let login = Arc::new(Active {
            view: Mutex::new(view.clone()),
            cancel: AtomicBool::new(false),
        });
        *active = Some(Arc::clone(&login));
        drop(active);
        let http = self.http.clone();
        let endpoints = self.endpoints.clone();
        let auth_file = self.auth_file.clone();
        tokio::spawn(async move {
            run(
                http,
                endpoints,
                auth_file,
                login,
                device_auth_id,
                user_code,
                interval,
            )
            .await;
        });
        Ok(view)
    }

    pub async fn status(&self, id: &str) -> Option<LoginView> {
        let active = self.active.lock().await;
        let login = active.as_ref()?;
        let view = login.view.lock().await.clone();
        (view.id == id).then_some(view)
    }

    pub async fn cancel(&self, id: &str) -> Result<(), LoginError> {
        let active = self.active.lock().await;
        let Some(login) = active.as_ref() else {
            return Err(LoginError::not_found("the ChatGPT login is not running"));
        };
        let mut view = login.view.lock().await;
        if view.id != id {
            return Err(LoginError::not_found("the ChatGPT login is not running"));
        }
        view.state = "canceled".into();
        login.cancel.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub async fn sign_out(&self) -> Result<(), LoginError> {
        let mut auth = read_auth(&self.auth_file);
        let Some(object) = auth.as_object_mut() else {
            return Err(LoginError::not_found("ChatGPT is not signed in"));
        };
        if object.remove("openai-codex").is_none() {
            return Err(LoginError::not_found("ChatGPT is not signed in"));
        }
        save_auth(&self.auth_file, &auth)?;
        if let Some(login) = self.active.lock().await.as_ref() {
            login.cancel.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

async fn run(
    http: reqwest::Client,
    endpoints: CodexEndpoints,
    auth_file: PathBuf,
    login: Arc<Active>,
    device_auth_id: String,
    user_code: String,
    interval: f64,
) {
    let result = poll_and_exchange(
        &http,
        &endpoints,
        &login,
        &device_auth_id,
        &user_code,
        interval,
    )
    .await;
    let canceled = login.cancel.load(Ordering::SeqCst);
    let mut view = login.view.lock().await;
    if canceled {
        view.state = "canceled".into();
        return;
    }
    match result {
        Ok((access, refresh, expires_in)) => {
            match save_codex_auth(&auth_file, access, refresh, expires_in) {
                Ok(()) => {
                    view.state = "succeeded".into();
                    view.user_code.clear();
                    view.verification_uri.clear();
                }
                Err(error) => {
                    view.state = "failed".into();
                    view.error = error.message;
                }
            }
        }
        Err(error) => {
            view.state = "failed".into();
            view.error = error.message;
        }
    }
}

// Production-only wait on the provider's polling interval; the tests answer on the first poll.
#[allow(clippy::disallowed_methods)]
async fn poll_and_exchange(
    http: &reqwest::Client,
    endpoints: &CodexEndpoints,
    login: &Active,
    device_auth_id: &str,
    user_code: &str,
    mut interval: f64,
) -> Result<(String, String, u64), LoginError> {
    let deadline = Instant::now() + Duration::from_secs(DEVICE_LIFETIME_SECS);
    let (authorization_code, code_verifier) = loop {
        if login.cancel.load(Ordering::SeqCst) {
            return Err(LoginError::bad_request("ChatGPT login canceled"));
        }
        if Instant::now() >= deadline {
            return Err(LoginError::bad_request("the ChatGPT login expired"));
        }
        tokio::time::sleep(Duration::from_secs_f64(interval.max(0.25))).await;
        if login.cancel.load(Ordering::SeqCst) {
            return Err(LoginError::bad_request("ChatGPT login canceled"));
        }
        let answer = post_json(
            http,
            &endpoints.device_token,
            json!({ "device_auth_id": device_auth_id, "user_code": user_code }),
        )
        .await?;
        if answer.status.is_success() {
            let code = answer.value["authorization_code"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let verifier = answer.value["code_verifier"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if code.is_empty() || verifier.is_empty() {
                return Err(LoginError::server(
                    "ChatGPT device token response is missing its code",
                ));
            }
            break (code, verifier);
        }
        if answer.status.as_u16() == 403 || answer.status.as_u16() == 404 {
            continue;
        }
        match error_code(&answer.value).as_deref() {
            Some("deviceauth_authorization_pending") => continue,
            Some("slow_down") => {
                interval += 5.0;
                continue;
            }
            _ => {
                return Err(LoginError::server(format!(
                    "ChatGPT device login failed with status {}: {}",
                    answer.status,
                    answer.text.trim()
                )));
            }
        }
    };
    let form = format!(
        "grant_type=authorization_code&client_id={}&code={}&code_verifier={}&redirect_uri={}",
        CLIENT_ID,
        form_encode(&authorization_code),
        form_encode(&code_verifier),
        form_encode(REDIRECT_URI)
    );
    let response = http
        .post(&endpoints.exchange)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form)
        .send()
        .await
        .map_err(LoginError::server)?;
    let status = response.status();
    let text = response.text().await.map_err(LoginError::server)?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(LoginError::server(format!(
            "ChatGPT token exchange failed with status {status}: {}",
            text.trim()
        )));
    }
    let access = value["access_token"]
        .as_str()
        .ok_or_else(|| LoginError::server("ChatGPT token exchange returned no access token"))?
        .to_string();
    let refresh = value["refresh_token"]
        .as_str()
        .ok_or_else(|| LoginError::server("ChatGPT token exchange returned no refresh token"))?
        .to_string();
    let expires_in = value["expires_in"]
        .as_u64()
        .or_else(|| {
            value["expires_in"]
                .as_str()
                .and_then(|text| text.parse().ok())
        })
        .ok_or_else(|| LoginError::server("ChatGPT token exchange returned no expiry"))?;
    Ok((access, refresh, expires_in))
}

struct Answer {
    status: reqwest::StatusCode,
    value: Value,
    text: String,
}

async fn post_json(http: &reqwest::Client, url: &str, body: Value) -> Result<Answer, LoginError> {
    let response = http
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(LoginError::server)?;
    let status = response.status();
    let text = response.text().await.map_err(LoginError::server)?;
    let value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(Answer {
        status,
        value,
        text,
    })
}

fn error_code(value: &Value) -> Option<String> {
    match value.get("error") {
        Some(Value::String(code)) => Some(code.clone()),
        Some(Value::Object(object)) => object
            .get("code")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn interval_of(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
}

fn save_codex_auth(
    path: &Path,
    access: String,
    refresh: String,
    expires_in: u64,
) -> Result<(), LoginError> {
    let mut auth = read_auth(path);
    let Some(object) = auth.as_object_mut() else {
        return Err(LoginError::server("the pi auth file is not an object"));
    };
    object.insert(
        "openai-codex".into(),
        json!({
            "type": "oauth",
            "access": access,
            "refresh": refresh,
            "expires": now_ms() + expires_in * 1000,
        }),
    );
    save_auth(path, &auth)
}

fn read_auth(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_auth(path: &Path, auth: &Value) -> Result<(), LoginError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(LoginError::server)?;
    }
    let text = serde_json::to_string_pretty(auth).map_err(LoginError::server)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(LoginError::server)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_millis() as u64)
        .unwrap_or_default()
}

fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_accepts_number_and_string() {
        assert_eq!(interval_of(&json!("5")), Some(5.0));
        assert_eq!(interval_of(&json!(3)), Some(3.0));
        assert_eq!(interval_of(&json!(-1)), None);
    }

    #[test]
    fn form_values_are_percent_encoded() {
        assert_eq!(form_encode("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn auth_round_trips_and_keeps_other_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pi/agent/auth.json");
        save_auth(&path, &json!({"opencode-go": {"access": "keep"}})).unwrap();
        let login = CodexLogin::new(&path);
        assert!(!login.signed_in());
        save_codex_auth(&path, "at".into(), "rt".into(), 60).unwrap();
        assert!(login.signed_in());
        let auth = read_auth(&path);
        assert_eq!(auth["opencode-go"]["access"], "keep");
        assert_eq!(auth["openai-codex"]["type"], "oauth");
    }
}
