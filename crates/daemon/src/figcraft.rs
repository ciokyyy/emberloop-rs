//! The `figcraft` MCP server entry in `<config root>/mcp.json`.

use serde_json::Value;
use std::path::Path;

/// Write the `figcraft` entry into `<config root>/mcp.json` when absent.
pub fn ensure(config_root: &Path) -> bool {
    let file = config_root.join("mcp.json");
    let Some(mut config) = read(&file) else {
        return false;
    };
    match insert(&mut config, config_root) {
        None => false,
        Some(false) => enabled(&config),
        Some(true) => {
            if let Err(error) = write(&file, &config) {
                tracing::warn!(%error, path = %file.display(), "cannot write mcp.json");
                return false;
            }
            enabled(&config)
        }
    }
}

fn read(file: &Path) -> Option<Value> {
    match std::fs::read_to_string(file) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(config) => Some(config),
            Err(error) => {
                tracing::warn!(%error, path = %file.display(), "mcp.json is not JSON; leaving it alone");
                None
            }
        },
        Err(_) => Some(serde_json::json!({})),
    }
}

fn insert(config: &mut Value, config_root: &Path) -> Option<bool> {
    let servers = config.as_object_mut().and_then(|root| {
        root.entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
    });
    let Some(servers) = servers else {
        tracing::warn!("mcpServers in mcp.json is not an object; leaving it alone");
        return None;
    };
    let entry = servers
        .entry("figcraft")
        .or_insert_with(tools::figcraft::default_entry);
    let before = entry.clone();
    let env = entry
        .as_object_mut()?
        .entry("env")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()?;
    env.insert(
        "FIGCRAFT_LIVE_DIR".into(),
        serde_json::json!(config_root.join("figcraft/projects")),
    );
    env.insert(
        "FIGCRAFT_SERVER_DATA".into(),
        serde_json::json!(config_root.join("figcraft/server")),
    );
    Some(*entry != before)
}

fn enabled(config: &Value) -> bool {
    config
        .pointer("/mcpServers/figcraft/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn write(file: &Path, config: &Value) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(config)?;
    let tmp = file.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file)
}

/// Keep the native collaboration server alive for the daemon's lifetime.
pub async fn start(config_root: &Path) -> anyhow::Result<tokio::process::Child> {
    drop(std::net::TcpListener::bind("127.0.0.1:47900")?);
    let data = config_root.join("figcraft/server");
    std::fs::create_dir_all(&data)?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data.join("server.log"))?;
    let mut child = tokio::process::Command::new("figcraft-server")
        .args(["serve", "--listen", "127.0.0.1:47900", "--data"])
        .arg(&data)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .kill_on_drop(true)
        .spawn()?;
    let mut readiness = tokio::time::interval(std::time::Duration::from_millis(25));
    for _ in 0..100 {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!(
                "FigCraft server exited: {status}; see {}",
                data.join("server.log").display()
            );
        }
        if reqwest::get("http://127.0.0.1:47900/healthz")
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return Ok(child);
        }
        readiness.tick().await;
    }
    anyhow::bail!("FigCraft server did not become ready")
}
