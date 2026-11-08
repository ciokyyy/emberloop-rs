use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Notify};
use tokio::time::timeout;

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
    /// Unnumbered events published and not yet stored; counted only while a persister runs.
    unstored: AtomicUsize,
    persisting: AtomicBool,
    stored: Notify,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            unstored: AtomicUsize::new(0),
            persisting: AtomicBool::new(false),
            stored: Notify::new(),
        }
    }

    pub fn publish(&self, event: SessionEvent) {
        if event.seq == 0 && self.persisting.load(Ordering::SeqCst) {
            self.unstored.fetch_add(1, Ordering::SeqCst);
        }
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.sender.subscribe()
    }

    /// A persister that numbers every unnumbered event is subscribed: from now on the bus counts
    /// them, so a writer that numbers its own events can wait for the backlog (`settled`).
    pub fn persister_started(&self) {
        self.persisting.store(true, Ordering::SeqCst);
    }

    /// The persister numbered one unnumbered event.
    pub fn event_stored(&self) {
        let _ = self
            .unstored
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        self.stored.notify_waiters();
    }

    /// The persister's receiver lagged: the events it missed will never be stored.
    pub fn events_lost(&self) {
        self.unstored.store(0, Ordering::SeqCst);
        self.stored.notify_waiters();
    }

    /// Wait until every unnumbered event published so far is numbered, so an event a writer
    /// numbers itself lands after them. `false` when `limit` ran out first.
    pub async fn settled(&self, limit: Duration) -> bool {
        timeout(limit, async {
            loop {
                let stored = self.stored.notified();
                tokio::pin!(stored);
                stored.as_mut().enable();
                if self.unstored.load(Ordering::SeqCst) == 0 {
                    return;
                }
                stored.await;
            }
        })
        .await
        .is_ok()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}
