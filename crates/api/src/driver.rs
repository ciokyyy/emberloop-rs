use async_trait::async_trait;

use crate::routes::PromptImageDto;

/// The daemon's turn loop, reached from the API. The API records events and
/// queues steers itself; everything that touches the harness goes through here.
#[async_trait]
pub trait SessionDriver: Send + Sync {
    async fn prompt(
        &self,
        session_id: &str,
        text: &str,
        images: Vec<PromptImageDto>,
    ) -> Result<(), String>;
    async fn cancel(&self, session_id: &str) -> Result<(), String>;
    async fn wake(
        &self,
        session_id: &str,
        text: &str,
        images: Vec<PromptImageDto>,
    ) -> Result<(), String>;
    /// Start the Session without sending a prompt, so it can take a steer.
    async fn connect(&self, session_id: &str) -> Result<(), String>;
    /// Apply a model and effort on an idle conversation. Failures must reach the caller.
    async fn configure_model(
        &self,
        _session_id: &str,
        _model: &str,
        _effort: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
    /// Update the thinking effort (`thought_level`) on a live Session if running.
    async fn set_thought_level(&self, _session_id: &str, _level: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Used by tests and by any API router built without a daemon.
pub struct NoopDriver;

#[async_trait]
impl SessionDriver for NoopDriver {
    async fn prompt(
        &self,
        _session_id: &str,
        _text: &str,
        _images: Vec<PromptImageDto>,
    ) -> Result<(), String> {
        Err("no session driver attached".into())
    }

    async fn cancel(&self, _session_id: &str) -> Result<(), String> {
        Err("no session driver attached".into())
    }

    async fn wake(
        &self,
        _session_id: &str,
        _text: &str,
        _images: Vec<PromptImageDto>,
    ) -> Result<(), String> {
        Err("no session driver attached".into())
    }

    async fn connect(&self, _session_id: &str) -> Result<(), String> {
        Err("no session driver attached".into())
    }
}
