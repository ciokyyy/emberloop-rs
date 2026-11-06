#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
use agent_client_protocol::v1::schema::McpServer;
use agent_client_protocol::AcpAgentConfig;
use async_trait::async_trait;
use harness::{Harness, HarnessError, HarnessSession, HarnessWiring};
use harness_acp::{start, AcpLaunch, AcpSession};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The one provider phase 2 wires: pi's built-in OpenCode Go catalogue.
const PROVIDER: &str = "opencode-go";

/// `pi --mode acp` (pi-rust). Each Session gets its own `HOME` under `homes`
/// holding `models.json` (proxy base URL and key) and `mcp.json` (the tools);
/// pi starts with a scrubbed environment so it never reads the User's keys.
pub struct PiHarness {
    bin: PathBuf,
    homes: PathBuf,
    keys: PathBuf,
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
        }
    }

    fn key(&self) -> Result<String, HarnessError> {
        let missing =
            || HarnessError::Process(format!("no {PROVIDER} key in {}", self.keys.display()));
        let text = std::fs::read_to_string(&self.keys).map_err(|_| missing())?;
        let keys: serde_json::Value = serde_json::from_str(&text)?;
        keys[PROVIDER]
            .as_str()
            .filter(|key| !key.is_empty())
            .map(str::to_string)
            .ok_or_else(missing)
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
    ) -> Result<AcpSession, HarnessError> {
        let key = self.key()?;
        let home = self.homes.join(session_id);
        let cwd = wiring.workdir.clone().unwrap_or_else(|| home.join("work"));
        write_session_files(&home, &cwd, &wiring, &key)?;
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
        let session = self.open("_catalogue", wiring).await?;
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
        Ok(Box::new(self.open(session_id, wiring).await?))
    }
}

fn write_session_files(
    home: &Path,
    cwd: &Path,
    wiring: &HarnessWiring,
    key: &str,
) -> std::io::Result<()> {
    let agent = home.join(".pi/agent");
    std::fs::create_dir_all(&agent)?;
    std::fs::create_dir_all(cwd)?;
    // pi appends `/chat/completions` etc. to the base; the proxy routes on that suffix.
    let models = serde_json::json!({"providers": {PROVIDER: {
        "baseUrl": format!("{}/v1", wiring.proxy_url.trim_end_matches('/')),
        "apiKey": key,
    }}});
    write_private(&agent.join("models.json"), &models.to_string())?;
    write_private(&agent.join("mcp.json"), &mcp_config(wiring).to_string())
}

/// pi's own `mcp.json`: ACP `session/new` refuses HTTP MCP, this file does not.
fn mcp_config(wiring: &HarnessWiring) -> serde_json::Value {
    let server = match (&wiring.mcp_url, &wiring.mcp_command) {
        (Some(url), _) => serde_json::json!({"url": url, "exposure": "direct"}),
        (None, Some(command)) if !command.is_empty() => serde_json::json!({
            "command": command[0], "args": &command[1..], "exposure": "direct",
        }),
        _ => return serde_json::json!({"mcpServers": {}}),
    };
    serde_json::json!({"mcpServers": {"el": server}})
}

fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(contents.as_bytes())
}
