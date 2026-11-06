use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Status,
    SessionTitle,
    Prompt,
    AssistantChunk,
    ThinkingChunk,
    ToolStarted,
    ToolFinished,
    SteerQueued,
    SteerDelivered,
    Notice,
    Todos,
    Compression,
    Usage,
    Error,
    Timing,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub v: u32,
    pub seq: u64,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub kind: EventKind,
    pub at: u64,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl SessionEvent {
    pub fn new(
        session_id: impl Into<String>,
        seq: u64,
        kind: EventKind,
        data: serde_json::Value,
    ) -> Self {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            v: 1,
            seq,
            session_id: session_id.into(),
            kind,
            at,
            data,
        }
    }
}

pub struct EventBus {
    sender: broadcast::Sender<SessionEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn publish(&self, event: SessionEvent) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.sender.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}
