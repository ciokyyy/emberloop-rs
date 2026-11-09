//! Durable project designs shared by FigCraft Web and agent MCP clients.
use axum::{
    extract::ws::{Message, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;
use tools::{mcpclient::McpRegistry, ToolError, ToolResult};

#[derive(Default)]
pub struct Designs {
    config: Option<PathBuf>,
    clients: Mutex<HashMap<PathBuf, Arc<McpRegistry>>>,
}

impl Designs {
    pub fn new(config: PathBuf) -> Self {
        Self {
            config: Some(config),
            ..Self::default()
        }
    }
    pub async fn remove(&self, root: &Path) {
        if let Ok(root) = std::fs::canonicalize(root) {
            self.clients.lock().await.remove(&root);
        }
    }
    pub async fn link(&self, root: &Path) -> ToolResult<String> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| ToolError::msg("FigCraft live server is not configured"))?;
        let root = std::fs::canonicalize(root)?;
        let mut clients = self.clients.lock().await;
        let client = if let Some(client) = clients.get(&root) {
            Arc::clone(client)
        } else {
            let client = Arc::new(McpRegistry::load(&root, Some(config)));
            client.figcraft_link().await?;
            clients.insert(root.to_owned(), Arc::clone(&client));
            let backup = Arc::downgrade(&client);
            let root = root.to_owned();
            tokio::spawn(async move {
                let mut timer = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    timer.tick().await;
                    let Some(backup) = backup.upgrade() else {
                        break;
                    };
                    if let Err(error) = snapshot(&backup, &root).await {
                        eprintln!("FigCraft project backup failed: {error}");
                    }
                }
            });
            client
        };
        client.figcraft_link().await
    }
}

async fn snapshot(client: &McpRegistry, root: &Path) -> ToolResult<()> {
    let temp = root.join(".design.backup.figcraft");
    client
        .call("figcraft.save_file", &json!({"path":temp}))
        .await?;
    tokio::fs::rename(temp, tools::figcraft::design_path(root)).await?;
    Ok(())
}

// Only same-origin browser clients may reach the private loopback collaboration server.
pub(crate) async fn live(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, &'static str)> {
    if !same_origin(&headers) {
        return Err((StatusCode::FORBIDDEN, "same-origin connection required"));
    }
    // FigCraft authenticates with a binary Hello, not WebSocket query parameters.
    let (upstream, _) = tokio_tungstenite::connect_async("ws://127.0.0.1:47900/")
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "FigCraft server unavailable",
            )
        })?;
    Ok(ws.on_upgrade(move |browser| async move {
        let (mut browser_out, mut browser_in) = browser.split();
        let (mut server_out, mut server_in) = upstream.split();
        let send = async {
            while let Some(Ok(message)) = browser_in.next().await {
                if server_out.send(to_server(message)).await.is_err() {
                    break;
                }
            }
        };
        let receive = async {
            while let Some(Ok(message)) = server_in.next().await {
                if let Some(message) = to_browser(message) {
                    if browser_out.send(message).await.is_err() {
                        break;
                    }
                }
            }
        };
        tokio::select! { () = send => {}, () = receive => {} }
    }))
}

fn same_origin(headers: &HeaderMap) -> bool {
    let origin = headers.get("origin").and_then(|s| s.to_str().ok());
    let host = headers.get("host").and_then(|s| s.to_str().ok());
    match (origin, host) {
        (Some(origin), Some(host)) => {
            origin
                .strip_prefix("http://")
                .or_else(|| origin.strip_prefix("https://"))
                == Some(host)
        }
        _ => false,
    }
}

fn to_server(message: Message) -> tokio_tungstenite::tungstenite::Message {
    use tokio_tungstenite::tungstenite::Message as ServerMessage;
    match message {
        Message::Text(text) => ServerMessage::Text(text.to_string().into()),
        Message::Binary(bytes) => ServerMessage::Binary(bytes),
        Message::Ping(bytes) => ServerMessage::Ping(bytes),
        Message::Pong(bytes) => ServerMessage::Pong(bytes),
        Message::Close(_) => ServerMessage::Close(None),
    }
}

fn to_browser(message: tokio_tungstenite::tungstenite::Message) -> Option<Message> {
    use tokio_tungstenite::tungstenite::Message as ServerMessage;
    match message {
        ServerMessage::Text(text) => Some(Message::Text(text.to_string().into())),
        ServerMessage::Binary(bytes) => Some(Message::Binary(bytes)),
        ServerMessage::Ping(bytes) => Some(Message::Ping(bytes)),
        ServerMessage::Pong(bytes) => Some(Message::Pong(bytes)),
        ServerMessage::Close(_) => Some(Message::Close(None)),
        ServerMessage::Frame(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_collaboration_rejects_cross_origin_and_missing_origin() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "localhost:3000".parse().unwrap());
        assert!(!same_origin(&headers));
        headers.insert("origin", "https://attacker.example".parse().unwrap());
        assert!(!same_origin(&headers));
        headers.insert("origin", "http://localhost:3000".parse().unwrap());
        assert!(same_origin(&headers));
    }
}
