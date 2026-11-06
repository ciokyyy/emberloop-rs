use std::time::{Duration, Instant};

/// Buffers assistant text until the window elapses, a tool starts, or the turn
/// ends. Callers flush with `take` at a tool boundary or turn end.
pub struct EventCoalescer {
    window: Duration,
    started: Option<Instant>,
    chunks: usize,
    text: String,
}

impl EventCoalescer {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            started: None,
            chunks: 0,
            text: String::new(),
        }
    }

    /// Returns true when the window is due; callers flush then, or earlier at a
    /// tool boundary.
    pub fn push(&mut self, text: &str, now: Instant) -> bool {
        if self.started.is_none() {
            self.started = Some(now);
        }
        self.text.push_str(text);
        self.chunks += 1;
        now.duration_since(self.started.unwrap_or(now)) >= self.window
    }

    pub fn take(&mut self) -> Option<String> {
        if self.text.is_empty() {
            return None;
        }
        self.started = None;
        self.chunks = 0;
        Some(std::mem::take(&mut self.text))
    }

    pub fn chunks(&self) -> usize {
        self.chunks
    }
}

use crate::event_log::EventLogError;
use crate::EventLog;
use emberloop_core::events::{EventKind, SessionEvent};

/// Buffers chunks and writes one AssistantChunk event on flush.
pub struct CoalescedWriter {
    log: EventLog,
    session_id: String,
    coalescer: EventCoalescer,
}

impl CoalescedWriter {
    pub fn new(log: EventLog, session_id: impl Into<String>, window: Duration) -> Self {
        Self {
            log,
            session_id: session_id.into(),
            coalescer: EventCoalescer::new(window),
        }
    }

    pub async fn push(
        &mut self,
        text: &str,
        now: Instant,
    ) -> Result<Option<SessionEvent>, EventLogError> {
        if self.coalescer.push(text, now) {
            self.flush().await
        } else {
            Ok(None)
        }
    }

    pub async fn flush(&mut self) -> Result<Option<SessionEvent>, EventLogError> {
        let Some(text) = self.coalescer.take() else {
            return Ok(None);
        };
        let event = SessionEvent::new(
            &self.session_id,
            0,
            EventKind::AssistantChunk,
            serde_json::json!({"text": text}),
        );
        Ok(Some(self.log.append(event).await?))
    }
}
