//! Join the same durable FigCraft document from every project-scoped MCP client.
use super::*;
use sha2::{Digest, Sha256};

impl McpRegistry {
    fn design_metadata(&self, server: &Server) -> ToolResult<PathBuf> {
        let dir = server.entry.env.get("FIGCRAFT_LIVE_DIR")
            .ok_or_else(|| ToolError::msg("FigCraft live server is not configured; run scripts/install-figcraft and restart the daemon"))?;
        let root = std::fs::canonicalize(&self.root)?;
        Ok(Path::new(dir).join(format!(
            "{}.json",
            hex::encode(Sha256::digest(root.as_os_str().as_encoded_bytes()))
        )))
    }

    /// The private edit link for the project's one live document.
    pub async fn figcraft_link(&self) -> ToolResult<String> {
        self.ensure(figcraft::SERVER).await?;
        let server = self
            .servers
            .iter()
            .find(|s| s.name == figcraft::SERVER)
            .ok_or_else(|| ToolError::msg("FigCraft live server is not configured"))?;
        let text = std::fs::read_to_string(self.design_metadata(server)?)?;
        let value: Value = serde_json::from_str(&text)?;
        value["link"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ToolError::msg("FigCraft live document has no edit link"))
    }

    pub(super) async fn connect_design(
        &self,
        server: &Server,
        ready: &Connected,
    ) -> ToolResult<()> {
        let metadata = self.design_metadata(server)?;
        let parent = metadata
            .parent()
            .ok_or_else(|| ToolError::msg("invalid FigCraft metadata path"))?;
        std::fs::create_dir_all(parent)?;
        let lock_path = metadata.with_extension("lock");
        // A file lock also coordinates MCP processes belonging to different agent Sessions.
        let lock = tokio::task::spawn_blocking(move || -> std::io::Result<std::fs::File> {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(lock_path)?;
            file.lock()?;
            Ok(file)
        })
        .await
        .map_err(|e| ToolError::msg(e.to_string()))??;
        let link = match std::fs::read_to_string(&metadata) {
            Ok(text) => {
                let value: Value = serde_json::from_str(&text)?;
                let link = value["link"]
                    .as_str()
                    .ok_or_else(|| ToolError::msg("invalid FigCraft live metadata"))?;
                let joined = async {
                    command(
                        server,
                        ready,
                        "collab.join",
                        serde_json::json!({"link": link}),
                    )
                    .await?;
                    wait_live(server, ready).await
                }
                .await;
                if joined.is_ok() {
                    link.to_string()
                } else {
                    // The server forgot this link (restart with an empty store); publish the disk document again.
                    self.publish_design(server, ready).await?
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.publish_design(server, ready).await?
            }
            Err(e) => return Err(e.into()),
        };
        let tmp = metadata.with_extension(format!("tmp.{}", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options.open(&tmp)?;
        file.write_all(serde_json::json!({"link": link}).to_string().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(tmp, metadata)?;
        drop(lock);
        Ok(())
    }

    async fn publish_design(&self, server: &Server, ready: &Connected) -> ToolResult<String> {
        let data = server
            .entry
            .env
            .get("FIGCRAFT_SERVER_DATA")
            .ok_or_else(|| {
                ToolError::msg("FigCraft live server data directory is not configured")
            })?;
        let key = tokio::process::Command::new("figcraft-server")
            .args(["key", "--data", data])
            .output()
            .await?;
        if !key.status.success() {
            return Err(ToolError::msg("cannot read FigCraft live server key"));
        }
        let text = String::from_utf8_lossy(&key.stdout);
        let address = text
            .split_whitespace()
            .find(|word| word.starts_with("ws://") || word.starts_with("wss://"))
            .ok_or_else(|| ToolError::msg("FigCraft server key output has no publish address"))?;
        let path = figcraft::ensure_design_file(&self.root)?;
        raw_call(server, ready, "open_file", serde_json::json!({"path":path})).await?;
        command(server, ready, "collab.publish", serde_json::json!({"server":address, "name":self.root.file_name().unwrap_or_default().to_string_lossy()})).await?;
        let peers = wait_live(server, ready).await?;
        peers
            .pointer("/published/edit")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ToolError::msg("FigCraft server did not return an edit link"))
    }
}

async fn command(
    server: &Server,
    ready: &Connected,
    name: &str,
    params: Value,
) -> ToolResult<Value> {
    let output = raw_call(
        server,
        ready,
        "run_command",
        serde_json::json!({"command":name,"params":params}),
    )
    .await?;
    serde_json::from_str(&output).map_err(Into::into)
}

impl McpRegistry {
    /// A live session that dropped (the server restarted, the link was lost) never comes back by
    /// itself, so every later call fails. Join the stored link again once, then sync.
    pub(super) async fn resync(&self, server: &Server, ready: &Connected) -> ToolResult<()> {
        match sync(server, ready).await {
            Err(e) if e.to_string().starts_with("FigCraft live session failed") => {
                self.connect_design(server, ready).await?;
                sync(server, ready).await
            }
            other => other,
        }
    }
}

// Inspect and pointer tools do not pump upstream collaboration themselves.
pub(super) async fn sync(server: &Server, ready: &Connected) -> ToolResult<()> {
    wait_live(server, ready).await.map(|_| ())
}

async fn wait_live(server: &Server, ready: &Connected) -> ToolResult<Value> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut progress = tokio::time::interval(Duration::from_millis(25));
        loop {
            let peers = command(server, ready, "collab.peers", serde_json::json!({})).await?;
            match peers["status"].as_str() {
                Some("live") => return Ok(peers),
                Some("failed" | "off") => {
                    return Err(ToolError::msg(format!(
                        "FigCraft live session failed: {}",
                        peers["reason"]
                    )))
                }
                _ => {
                    progress.tick().await;
                }
            }
        }
    })
    .await
    .map_err(|_| ToolError::msg("FigCraft live session connection timed out"))?
}
