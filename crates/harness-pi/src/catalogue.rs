//! Read pi's own model metadata and model-specific thinking levels, without inference traffic.
use super::{write_session_files, PiHarness};
use harness::{HarnessError, HarnessWiring};
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

#[derive(Debug)]
pub struct ModelDetails {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub efforts: Vec<String>,
    pub image_input: bool,
    pub context_window: usize,
}

struct Rpc {
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    seq: u64,
}

impl Rpc {
    async fn call(&mut self, mut command: Value) -> Result<Value, HarnessError> {
        self.seq += 1;
        let id = format!("catalogue-{}", self.seq);
        command["id"] = json!(id);
        let line = format!("{command}\n");
        self.input.write_all(line.as_bytes()).await?;
        let mut line = String::new();
        loop {
            line.clear();
            if self.output.read_line(&mut line).await? == 0 {
                return Err(HarnessError::Protocol("pi catalogue RPC closed".into()));
            }
            let response: Value = serde_json::from_str(&line)?;
            if response["id"] != id {
                continue;
            }
            if response["success"] != true {
                return Err(HarnessError::Protocol(format!(
                    "pi catalogue: {}",
                    response["error"]
                )));
            }
            return Ok(response["data"].clone());
        }
    }

    async fn entries(&mut self, providers: &[String]) -> Result<Vec<ModelDetails>, HarnessError> {
        let data = self.call(json!({"type":"get_available_models"})).await?;
        let models = data["models"]
            .as_array()
            .ok_or_else(|| HarnessError::Protocol("pi returned no model metadata".into()))?;
        let mut entries = Vec::new();
        for model in models {
            let Some(provider) = model["provider"].as_str() else {
                continue;
            };
            if !providers.iter().any(|id| id == provider)
                || model["type"].as_str().is_some_and(|kind| kind != "chat")
            {
                continue;
            }
            let Some(id) = model["id"].as_str() else {
                continue;
            };
            let efforts = self.efforts(model, provider, id).await?;
            entries.push(ModelDetails {
                id: format!("{provider}/{id}"),
                label: model["name"].as_str().unwrap_or(id).to_string(),
                provider: provider.to_string(),
                efforts,
                image_input: model["input"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item == "image")),
                context_window: model["contextWindow"].as_u64().unwrap_or(0) as usize,
            });
        }
        Ok(entries)
    }

    async fn efforts(
        &mut self,
        model: &Value,
        provider: &str,
        id: &str,
    ) -> Result<Vec<String>, HarnessError> {
        if model["reasoning"] != true {
            return Ok(Vec::new());
        }
        self.call(json!({"type":"set_model", "provider":provider, "modelId":id}))
            .await?;
        let data = self
            .call(json!({"type":"get_available_thinking_levels"}))
            .await?;
        Ok(data["levels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect())
    }
}

impl PiHarness {
    /// The runtime's metadata, not a static approximation of all models' capabilities.
    pub async fn catalogue(&self) -> Result<Vec<ModelDetails>, HarnessError> {
        let keys = self.keys()?;
        let mut providers: Vec<String> = keys.keys().cloned().collect();
        if self
            .auth
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|auth| auth.get("openai-codex").is_some())
        {
            providers.push("openai-codex".to_string());
        }
        if providers.is_empty() {
            return Ok(Vec::new());
        }
        let home = self.homes.join("_metadata");
        let cwd = home.join("work");
        let wiring = HarnessWiring {
            proxy_url: "http://127.0.0.1:9".into(),
            mcp_url: None,
            mcp_command: None,
            env: Vec::new(),
            resume_token: None,
            workdir: None,
            model: None,
            thought_level: None,
            system_prompt: None,
        };
        write_session_files(&home, &cwd, &wiring, &keys, self.auth.as_deref())?;
        let mut child = Command::new(&self.bin)
            .args(["--mode", "rpc", "--no-builtin-tools", "--offline"])
            .env_clear()
            .env("HOME", &home)
            .env(
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
            )
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut rpc = Rpc {
            input: child
                .stdin
                .take()
                .ok_or_else(|| HarnessError::Process("missing pi stdin".into()))?,
            output: BufReader::new(
                child
                    .stdout
                    .take()
                    .ok_or_else(|| HarnessError::Process("missing pi stdout".into()))?,
            ),
            seq: 0,
        };
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(30), rpc.entries(&providers)).await;
        let _ = child.kill().await;
        result.map_err(|_| HarnessError::Protocol("pi catalogue timed out".into()))?
    }
}
