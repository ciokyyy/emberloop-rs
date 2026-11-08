#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
pub mod catalogue;
use agent_client_protocol::v1::schema::McpServer;
use agent_client_protocol::AcpAgentConfig;
use async_trait::async_trait;
use emberloop_core::providers::{key_provider, KEY_PROVIDERS, OAUTH_PROVIDERS};
use harness::{Harness, HarnessError, HarnessSession, HarnessWiring};
use harness_acp::{start, AcpLaunch, AcpSession};
use std::collections::BTreeMap;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// pi's built-in OpenCode Go catalogue; its wires are routed by path suffix, not by `x/<id>`.
const OPENCODE_GO: &str = "opencode-go";

/// `pi --mode acp` (pi-rust). Each Session gets its own `HOME` under `homes`
/// holding `models.json` (proxy base URL and key) and `mcp.json` (the tools);
/// pi starts with a scrubbed environment so it never reads the User's keys.
pub struct PiHarness {
    bin: PathBuf,
    homes: PathBuf,
    keys: PathBuf,
    auth: Option<PathBuf>,
}

impl PiHarness {
    pub fn new(
        bin: impl Into<PathBuf>,
        homes: impl Into<PathBuf>,
        keys_file: impl Into<PathBuf>,
    ) -> Self {
        Self {
            bin: bin.into(),
            homes: homes.into(),
            keys: keys_file.into(),
            auth: None,
        }
    }

    pub fn with_auth(mut self, auth_file: impl Into<PathBuf>) -> Self {
        self.auth = Some(auth_file.into());
        self
    }

    /// The saved keys of providers the proxy can route; empty and unknown entries drop out.
    fn keys(&self) -> Result<BTreeMap<String, String>, HarnessError> {
        let Ok(text) = std::fs::read_to_string(&self.keys) else {
            return Ok(BTreeMap::new());
        };
        let saved: BTreeMap<String, serde_json::Value> = serde_json::from_str(&text)?;
        Ok(saved
            .into_iter()
            .filter(|(id, _)| id == OPENCODE_GO || key_provider(id).is_some())
            .filter_map(|(id, key)| {
                Some((id, key.as_str().filter(|key| !key.is_empty())?.to_string()))
            })
            .collect())
    }

    /// `env -i` clears what the daemon inherited; the key travels in
    /// `models.json`, not in argv or the environment.
    fn config(&self, home: &Path) -> AcpAgentConfig {
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string());
        AcpAgentConfig::new("env").args([
            "-i".to_string(),
            format!("HOME={}", home.display()),
            format!("PATH={path}"),
            self.bin.display().to_string(),
            "--mode".into(),
            "acp".into(),
            "--no-builtin-tools".into(),
            "--offline".into(),
        ])
    }
}

impl PiHarness {
    async fn open(
        &self,
        session_id: &str,
        wiring: HarnessWiring,
        required: Option<&str>,
    ) -> Result<AcpSession, HarnessError> {
        let keys = self.keys()?;
        if let Some(provider) = required.filter(|provider| !keys.contains_key(*provider)) {
            return Err(HarnessError::Process(format!(
                "no {provider} key in {}",
                self.keys.display()
            )));
        }
        let home = self.homes.join(session_id);
        let cwd = wiring.workdir.clone().unwrap_or_else(|| home.join("work"));
        write_session_files(&home, &cwd, &wiring, &keys, self.auth.as_deref())?;
        let launch = AcpLaunch {
            config: self.config(&home),
            cwd,
            mcp_servers: Vec::<McpServer>::new(),
            meta: None,
            model: wiring.model.filter(|model| !model.is_empty()),
            thought_level: wiring.thought_level.filter(|level| !level.is_empty()),
            client_name: "emberloop",
            label: "pi",
            forward_thoughts: false,
            resume: wiring.resume_token,
        };
        start(launch, session_id).await
    }

    /// The models pi offers, as `(id, label)`, from a throwaway `session/new`.
    pub async fn models(&self) -> Result<Vec<(String, String)>, HarnessError> {
        let wiring = HarnessWiring {
            // `--offline` pi makes no request, so the proxy is never reached.
            proxy_url: "http://127.0.0.1:9".into(),
            mcp_url: None,
            mcp_command: None,
            env: Vec::new(),
            model: None,
            thought_level: None,
            resume_token: None,
            workdir: None,
        };
        let session = self.open("_catalogue", wiring, None).await?;
        Ok(model_options(&session.config_options))
    }
}

/// The select values of the `model` config option, groups flattened.
pub fn model_options(config_options: &serde_json::Value) -> Vec<(String, String)> {
    let option = config_options
        .as_array()
        .and_then(|all| all.iter().find(|option| option["id"] == "model"));
    let mut found = Vec::new();
    for entry in option
        .and_then(|option| option["options"].as_array())
        .into_iter()
        .flatten()
    {
        let grouped = entry["options"].as_array();
        for item in grouped.map_or(std::slice::from_ref(entry), Vec::as_slice) {
            if let (Some(value), Some(name)) = (item["value"].as_str(), item["name"].as_str()) {
                found.push((value.to_string(), name.to_string()));
            }
        }
    }
    found
}

#[async_trait]
impl Harness for PiHarness {
    async fn spawn(
        &self,
        session_id: &str,
        wiring: HarnessWiring,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        // A model of a provider that needs a saved key (not Codex OAuth) needs that key.
        let required = match wiring.model.as_deref() {
            None => Some(OPENCODE_GO.to_string()),
            Some(model) => model
                .split_once('/')
                .map(|(provider, _)| provider)
                .filter(|provider| *provider == OPENCODE_GO || key_provider(provider).is_some())
                .map(str::to_string),
        };
        let session = self.open(session_id, wiring, required.as_deref()).await?;
        Ok(Box::new(session))
    }
}

fn write_session_files(
    home: &Path,
    cwd: &Path,
    wiring: &HarnessWiring,
    keys: &BTreeMap<String, String>,
    auth: Option<&Path>,
) -> std::io::Result<()> {
    let agent = home.join(".pi/agent");
    std::fs::create_dir_all(&agent)?;
    std::fs::create_dir_all(cwd)?;
    let models_path = agent.join("models.json");
    write_private(&models_path, &models_config(wiring, keys).to_string())?;
    let settings_path = agent.join("settings.json");
    let mut settings: serde_json::Value = std::fs::read(&settings_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    // The model-wire proxy sees full requests over HTTP/SSE, not cached WebSocket deltas.
    settings["transport"] = serde_json::json!("sse");
    write_private(&settings_path, &settings.to_string())?;
    if let Some(auth) = auth {
        match std::fs::read(auth) {
            Ok(bytes) => write_private_bytes(&agent.join("auth.json"), &bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::remove_file(agent.join("auth.json"));
            }
            Err(error) => return Err(error),
        }
    }
    write_private(&agent.join("mcp.json"), &mcp_config(wiring).to_string())
}

/// pi's `models.json`: every saved provider talks to the proxy, which holds no key of its own.
/// pi appends `/chat/completions` etc. to the base; the proxy routes on that suffix.
fn models_config(wiring: &HarnessWiring, keys: &BTreeMap<String, String>) -> serde_json::Value {
    let proxy = wiring.proxy_url.trim_end_matches('/');
    let mut providers: serde_json::Map<_, _> = keys
        .iter()
        .map(|(id, key)| {
            let base_url = match KEY_PROVIDERS.iter().find(|provider| provider.id == id) {
                Some(provider) => provider.proxied_base_url(proxy),
                None => format!("{proxy}/v1"),
            };
            (
                id.clone(),
                serde_json::json!({"baseUrl": base_url, "apiKey": key}),
            )
        })
        .collect();
    for provider in OAUTH_PROVIDERS {
        providers.insert(
            provider.id.to_string(),
            serde_json::json!({"baseUrl": provider.proxied_base_url(proxy)}),
        );
    }
    serde_json::json!({ "providers": providers })
}

/// pi's own `mcp.json`: ACP `session/new` refuses HTTP MCP, this file does not.
fn mcp_config(wiring: &HarnessWiring) -> serde_json::Value {
    // pi aborts an MCP call after 60 s by default (pig docs, mcp.md); pig writes 600 on its own el entry.
    const TIMEOUT_SECONDS: u64 = 600;
    let server = match (&wiring.mcp_url, &wiring.mcp_command) {
        (Some(url), _) => serde_json::json!({
            "url": url, "exposure": "direct", "timeout": TIMEOUT_SECONDS,
        }),
        (None, Some(command)) if !command.is_empty() => serde_json::json!({
            "command": command[0], "args": &command[1..], "exposure": "direct",
            "timeout": TIMEOUT_SECONDS,
        }),
        _ => return serde_json::json!({"mcpServers": {}}),
    };
    serde_json::json!({"mcpServers": {"el": server}})
}

fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    write_private_bytes(path, contents.as_bytes())
}

fn write_private_bytes(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(contents)
}
