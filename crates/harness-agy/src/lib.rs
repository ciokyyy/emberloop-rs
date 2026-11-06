#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
use agent_client_protocol::v1::schema::{EnvVariable, McpServer, McpServerStdio};
use agent_client_protocol::AcpAgentConfig;
use async_trait::async_trait;
use harness::{Harness, HarnessError, HarnessSession, HarnessWiring};
use harness_acp::{start, AcpLaunch};
use std::path::PathBuf;

pub struct AgyHarness {
    server_bin: PathBuf,
    account_home: PathBuf,
    default_model: String,
}

impl AgyHarness {
    pub fn new(server_bin: impl Into<PathBuf>, account_home: impl Into<PathBuf>) -> Self {
        Self {
            server_bin: server_bin.into(),
            account_home: account_home.into(),
            default_model: "gemini-3.8-flash".to_string(),
        }
    }

    /// The model a Session runs: its own choice, else the harness default.
    fn model_for(&self, wiring: &HarnessWiring) -> String {
        wiring
            .model
            .clone()
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| self.default_model.clone())
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    fn launch(&self, wiring: &HarnessWiring, cwd: PathBuf) -> AcpLaunch {
        AcpLaunch {
            config: self.config(wiring),
            cwd,
            mcp_servers: mcp_servers(wiring.mcp_command.clone()),
            meta: Some(session_meta()),
            model: Some(self.model_for(wiring)),
            thought_level: wiring
                .thought_level
                .clone()
                .filter(|level| !level.is_empty()),
            // The agy server lists Claude and GPT-OSS only to allowlisted
            // clients, so the harness announces itself as `zed`.
            client_name: "zed",
            label: "agy",
            forward_thoughts: true,
            resume: wiring.resume_token.clone(),
        }
    }

    fn config(&self, wiring: &HarnessWiring) -> AcpAgentConfig {
        let mut config = AcpAgentConfig::new(&self.server_bin)
            .arg("--uid=")
            .env("GEMINI_HOME", self.account_home.display().to_string())
            .env("AGY_ACP_CCPA_BASE_URL", &wiring.proxy_url);
        for (key, value) in &wiring.env {
            config = config.env(key, value);
        }
        config
    }
}

#[async_trait]
impl Harness for AgyHarness {
    async fn spawn(
        &self,
        session_id: &str,
        wiring: HarnessWiring,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        let cwd = match &wiring.workdir {
            Some(dir) => {
                std::fs::create_dir_all(dir).map_err(HarnessError::Io)?;
                dir.clone()
            }
            None => {
                let temp_dir = tempfile::tempdir().map_err(HarnessError::Io)?;
                temp_dir.keep()
            }
        };
        Ok(Box::new(
            start(self.launch(&wiring, cwd), session_id).await?,
        ))
    }
}

fn mcp_servers(command: Option<Vec<String>>) -> Vec<McpServer> {
    command
        .and_then(|mut parts| {
            let command = parts.first().cloned()?;
            let args = parts.split_off(1);
            Some(vec![McpServer::Stdio(
                McpServerStdio::new("el", command)
                    .args(args)
                    .env(Vec::<EnvVariable>::new()),
            )])
        })
        .unwrap_or_default()
}

fn session_meta() -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "antigravity".into(),
        serde_json::json!({
            "enabledTools": [],
            "eagerMcpServers": ["el"],
            "mcpMaxOutputBytes": 100000
        }),
    );
    meta
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sessions_model_wins_over_the_default() {
        let harness = AgyHarness::new("/bin/agy", "/tmp/agy-home");
        let mut wiring = HarnessWiring {
            proxy_url: String::new(),
            mcp_url: None,
            mcp_command: None,
            env: Vec::new(),
            model: Some("gemini-4-argon".into()),
            thought_level: None,
            resume_token: None,
            workdir: None,
        };
        assert_eq!(harness.model_for(&wiring), "gemini-4-argon");
        wiring.model = None;
        assert_eq!(harness.model_for(&wiring), "gemini-3.8-flash");
    }

    #[test]
    fn the_launch_identifies_as_an_allowlisted_client() {
        let harness = AgyHarness::new("/bin/agy", "/tmp/agy-home");
        let wiring = HarnessWiring {
            proxy_url: String::new(),
            mcp_url: None,
            mcp_command: None,
            env: Vec::new(),
            model: None,
            thought_level: None,
            resume_token: None,
            workdir: None,
        };
        let launch = harness.launch(&wiring, PathBuf::from("/tmp"));
        assert_eq!(launch.client_name, "zed");
        assert_eq!(launch.model.as_deref(), Some("gemini-3.8-flash"));
    }

    #[test]
    fn config_carries_confinement_and_proxy_wiring() {
        let harness = AgyHarness::new("/bin/agy", "/tmp/agy-home");
        let wiring = HarnessWiring {
            proxy_url: "http://127.0.0.1:9000/p/token".into(),
            mcp_url: None,
            mcp_command: Some(vec!["emberloop-mcp".into(), "--root".into(), "/p".into()]),
            env: vec![("EXTRA".into(), "1".into())],
            model: None,
            thought_level: None,
            resume_token: None,
            workdir: None,
        };
        let config = harness.config(&wiring);
        assert_eq!(config.command(), std::path::Path::new("/bin/agy"));
        assert_eq!(config.arguments(), ["--uid="]);
        assert_eq!(
            config.environment().get("GEMINI_HOME").map(String::as_str),
            Some("/tmp/agy-home")
        );
        assert_eq!(
            config
                .environment()
                .get("AGY_ACP_CCPA_BASE_URL")
                .map(String::as_str),
            Some("http://127.0.0.1:9000/p/token")
        );
        assert_eq!(
            config.environment().get("EXTRA").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn mcp_command_becomes_a_stdio_server() {
        let servers = mcp_servers(Some(vec!["emberloop-mcp".into(), "--root".into()]));
        assert!(matches!(servers.as_slice(), [McpServer::Stdio(server)]
            if server.name == "el" && server.args == ["--root"]));
    }
}
