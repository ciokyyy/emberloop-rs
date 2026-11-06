#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessWiring {
    pub proxy_url: String,
    pub mcp_url: Option<String>,
    pub mcp_command: Option<Vec<String>>,
    pub env: Vec<(String, String)>,
    /// The harness's id for this Session's conversation from an earlier run;
    /// the harness loads it, and starts a new one if it cannot.
    #[serde(default)]
    pub resume_token: Option<String>,
    /// A directory that stays the same for the Session's whole life, so a
    /// stored conversation loads against the directory it was made in.
    #[serde(default)]
    pub workdir: Option<std::path::PathBuf>,
    /// The Session's model; the harness default applies when unset.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptImage {
    pub mime_type: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum HarnessEvent {
    AssistantChunk(String),
    ToolStarted {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolFinished {
        id: String,
        output: serde_json::Value,
    },
    TurnFinished {
        stop_reason: Option<String>,
        usage: Option<serde_json::Value>,
    },
    Error(String),
}

pub type EventStream = Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>;

#[async_trait]
pub trait HarnessSession: Send + Sync {
    async fn send(&self, prompt: &str, images: Vec<PromptImage>) -> Result<(), HarnessError>;
    fn events(&self) -> EventStream;
    async fn cancel(&self) -> Result<(), HarnessError>;
    async fn resume(&self, token: &str) -> Result<(), HarnessError>;
    async fn wake(&self, text: &str) -> Result<(), HarnessError>;
    /// The id to store so a later run can load this conversation again.
    fn resume_token(&self) -> Option<String> {
        None
    }
    /// True when this run loaded the stored conversation instead of starting
    /// an empty one.
    fn resumed(&self) -> bool {
        false
    }
    /// Delete the harness's copy of this Session.
    async fn forget(&self, resume_token: &str) -> Result<(), HarnessError>;
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Process error: {0}")]
    Process(String),
    #[error("Protocol error: {0}")]
    Protocol(String),
}

#[async_trait]
pub trait Harness: Send + Sync {
    async fn spawn(
        &self,
        session_id: &str,
        wiring: HarnessWiring,
    ) -> Result<Box<dyn HarnessSession>, HarnessError>;
}
