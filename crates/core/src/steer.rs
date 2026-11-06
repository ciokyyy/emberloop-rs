use crate::events::{EventBus, EventKind, SessionEvent};
use crate::id::generate_id;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerKind {
    Steer,
    Notice,
}

impl SteerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Notice => "notice",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerState {
    Pending,
    InFlight,
    Delivered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerEntry {
    pub id: String,
    pub kind: SteerKind,
    pub text: String,
    pub state: SteerState,
    pub queued_at: u64,
    pub delivered_at: Option<u64>,
    pub route: Option<String>,
}

/// A per-Session queue. `gate` serialises the turn-end wake with a proxy request
/// beginning, so a wake never overlaps an in-flight upstream request.
pub struct SteerQueue {
    entries: AsyncMutex<Vec<SteerEntry>>,
    gate: AsyncMutex<()>,
    in_flight: AtomicBool,
    turn_active: AtomicBool,
    path: Option<PathBuf>,
}

impl Default for SteerQueue {
    fn default() -> Self {
        Self {
            entries: AsyncMutex::new(Vec::new()),
            gate: AsyncMutex::new(()),
            in_flight: AtomicBool::new(false),
            turn_active: AtomicBool::new(false),
            path: None,
        }
    }
}

impl SteerQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a queue persisted in the Session directory. An entry that was in
    /// flight when the daemon stopped is requeued.
    pub async fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut entries = load(&path);
        for entry in &mut entries {
            if entry.state == SteerState::InFlight {
                entry.state = SteerState::Pending;
            }
        }
        Self {
            entries: AsyncMutex::new(entries),
            gate: AsyncMutex::new(()),
            in_flight: AtomicBool::new(false),
            turn_active: AtomicBool::new(false),
            path: Some(path),
        }
    }

    pub fn session_path(session_dir: &Path) -> PathBuf {
        session_dir.join("steer_queue.json")
    }

    pub async fn enqueue(&self, kind: SteerKind, text: impl Into<String>) -> SteerEntry {
        self.enqueue_with_id(generate_id("steer"), kind, text).await
    }

    pub async fn enqueue_with_id(
        &self,
        id: impl Into<String>,
        kind: SteerKind,
        text: impl Into<String>,
    ) -> SteerEntry {
        let entry = SteerEntry {
            id: id.into(),
            kind,
            text: text.into(),
            state: SteerState::Pending,
            queued_at: now_ms(),
            delivered_at: None,
            route: None,
        };
        let snapshot = {
            let mut entries = self.entries.lock().await;
            entries.push(entry.clone());
            entries.clone()
        };
        self.save(&snapshot);
        entry
    }

    /// Mark every pending entry in flight before one upstream request carries
    /// the whole queue as a single prompt.
    pub async fn drain_pending(&self) -> Vec<SteerEntry> {
        let (drained, snapshot) = {
            let mut entries = self.entries.lock().await;
            let mut drained = Vec::new();
            for entry in entries.iter_mut() {
                if entry.state == SteerState::Pending {
                    entry.state = SteerState::InFlight;
                    drained.push(entry.clone());
                }
            }
            (drained, entries.clone())
        };
        self.save(&snapshot);
        drained
    }

    pub async fn mark_in_flight(&self, ids: &[String]) {
        let snapshot = {
            let mut entries = self.entries.lock().await;
            for entry in entries.iter_mut() {
                if ids.contains(&entry.id) {
                    entry.state = SteerState::InFlight;
                }
            }
            entries.clone()
        };
        self.save(&snapshot);
    }

    pub async fn mark_delivered(&self, ids: &[String], route: &str) {
        let now = now_ms();
        let snapshot = {
            let mut entries = self.entries.lock().await;
            for entry in entries.iter_mut() {
                if ids.contains(&entry.id) {
                    entry.state = SteerState::Delivered;
                    entry.delivered_at = Some(now);
                    entry.route = Some(route.to_string());
                }
            }
            entries.clone()
        };
        self.save(&snapshot);
    }

    pub async fn requeue(&self, ids: &[String]) {
        let snapshot = {
            let mut entries = self.entries.lock().await;
            for entry in entries.iter_mut() {
                if ids.contains(&entry.id) {
                    entry.state = SteerState::Pending;
                    entry.delivered_at = None;
                    entry.route = None;
                }
            }
            entries.clone()
        };
        self.save(&snapshot);
    }

    /// A request owns the Session until `end_request`. Delivery is decided at
    /// the response head.
    pub async fn begin_request(&self) {
        let _gate = self.gate.lock().await;
        self.in_flight.store(true, Ordering::SeqCst);
    }

    /// A turn is running from its prompt until the harness reports it over; a
    /// steer queued meanwhile waits for the proxy to inject it.
    pub fn set_turn_active(&self, active: bool) {
        self.turn_active.store(active, Ordering::SeqCst);
    }

    pub fn is_turn_active(&self) -> bool {
        self.turn_active.load(Ordering::SeqCst)
    }

    pub async fn end_request(&self) {
        let _gate = self.gate.lock().await;
        self.in_flight.store(false, Ordering::SeqCst);
    }

    /// A 2xx response head means the model saw the entries. A non-2xx means the
    /// request never reached it, so the entries go back to pending.
    pub async fn response_head(&self, ids: &[String], status: u16) {
        if (200..300).contains(&status) {
            self.mark_delivered(ids, "response head").await;
        } else {
            self.requeue(ids).await;
        }
    }

    pub async fn connection_failed(&self, ids: &[String]) {
        self.requeue(ids).await;
    }

    /// A stream that dies after the head still counts as delivered; the stored
    /// splice replays it.
    pub async fn stream_failed_after_head(&self, _ids: &[String]) {}

    /// Turn-end wake. It fires only when no request is in flight and the queue
    /// is not empty, and it drains the whole queue into one prompt.
    pub async fn wake_if_idle(
        &self,
        waker: &dyn SteerWaker,
        events: &EventBus,
        session_id: &str,
    ) -> Result<usize, String> {
        let _gate = self.gate.lock().await;
        if self.in_flight.load(Ordering::SeqCst) {
            return Ok(0);
        }
        let entries = self.drain_pending().await;
        if entries.is_empty() {
            return Ok(0);
        }
        let ids: Vec<String> = entries.iter().map(|entry| entry.id.clone()).collect();
        let text = entries
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if let Err(error) = waker.wake(&text).await {
            self.requeue(&ids).await;
            return Err(error);
        }
        self.mark_delivered(&ids, "woken").await;
        for entry in entries {
            events.publish(delivered_event(session_id, &entry, "woken"));
        }
        Ok(ids.len())
    }

    pub async fn pending_len(&self) -> usize {
        self.entries
            .lock()
            .await
            .iter()
            .filter(|entry| entry.state == SteerState::Pending)
            .count()
    }

    pub async fn entries(&self) -> Vec<SteerEntry> {
        self.entries.lock().await.clone()
    }

    fn save(&self, entries: &[SteerEntry]) {
        let Some(path) = &self.path else {
            return;
        };
        let Ok(text) = serde_json::to_string_pretty(entries) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

fn load(path: &Path) -> Vec<SteerEntry> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[derive(Clone, Default)]
pub struct SteerQueues {
    inner: Arc<Mutex<HashMap<String, Arc<SteerQueue>>>>,
    dir: Option<PathBuf>,
}

impl SteerQueues {
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            dir: Some(dir.into()),
        }
    }

    pub fn queue(&self, session_id: &str) -> Arc<SteerQueue> {
        let mut queues = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queues
            .entry(session_id.to_string())
            .or_insert_with(|| {
                let path = self
                    .dir
                    .as_ref()
                    .map(|dir| SteerQueue::session_path(&dir.join(session_id)));
                match path {
                    Some(path) => Arc::new(SteerQueue {
                        entries: AsyncMutex::new(load(&path)),
                        gate: AsyncMutex::new(()),
                        in_flight: AtomicBool::new(false),
                        turn_active: AtomicBool::new(false),
                        path: Some(path),
                    }),
                    None => Arc::new(SteerQueue::new()),
                }
            })
            .clone()
    }

    pub fn get(&self, session_id: &str) -> Option<Arc<SteerQueue>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()
    }
}

#[async_trait]
pub trait SteerWaker: Send + Sync {
    async fn wake(&self, text: &str) -> Result<(), String>;
}

/// Route 2: a turn is over with no tool call, so wake the harness once with
/// every pending entry as a single prompt. A failed wake puts the entries back.
pub async fn deliver_idle(
    queue: &SteerQueue,
    waker: &dyn SteerWaker,
    events: &EventBus,
    session_id: &str,
) -> Result<usize, String> {
    let _gate = queue.gate.lock().await;
    if queue.in_flight.load(Ordering::SeqCst) || queue.turn_active.load(Ordering::SeqCst) {
        return Ok(0);
    }
    let entries = queue.drain_pending().await;
    if entries.is_empty() {
        return Ok(0);
    }
    let ids: Vec<String> = entries.iter().map(|entry| entry.id.clone()).collect();
    let text = entries
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if let Err(error) = waker.wake(&text).await {
        queue.requeue(&ids).await;
        return Err(error);
    }
    queue.mark_delivered(&ids, "sent at idle").await;
    for entry in entries {
        events.publish(delivered_event(session_id, &entry, "sent at idle"));
    }
    Ok(ids.len())
}

pub fn delivered_event(session_id: &str, entry: &SteerEntry, route: &str) -> SessionEvent {
    SessionEvent::new(
        session_id,
        0,
        EventKind::SteerDelivered,
        serde_json::json!({
            "id": entry.id,
            "kind": entry.kind.as_str(),
            "text": entry.text,
            "route": route
        }),
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FakeWaker {
        fail: AtomicBool,
        calls: Mutex<Vec<String>>,
    }

    impl FakeWaker {
        fn new() -> Self {
            Self {
                fail: AtomicBool::new(false),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SteerWaker for FakeWaker {
        async fn wake(&self, text: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(text.to_string());
            if self.fail.load(Ordering::SeqCst) {
                Err("wake failed".into())
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn drain_marks_in_flight_and_requeue_restores_pending() {
        let queue = SteerQueue::new();
        let entry = queue.enqueue(SteerKind::Steer, "go left").await;
        assert_eq!(queue.pending_len().await, 1);

        let drained = queue.drain_pending().await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].state, SteerState::InFlight);
        assert_eq!(queue.pending_len().await, 0);

        queue.requeue(&[entry.id]).await;
        assert_eq!(queue.pending_len().await, 1);
        assert_eq!(queue.entries().await[0].state, SteerState::Pending);
    }

    #[tokio::test]
    async fn idle_delivery_wakes_once_for_the_whole_queue() {
        let queue = SteerQueue::new();
        queue.enqueue(SteerKind::Notice, "bg_1 exited").await;
        queue.enqueue(SteerKind::Steer, "go left").await;
        let events = EventBus::new(8);
        let mut rx = events.subscribe();
        let waker = FakeWaker::new();

        let delivered = deliver_idle(&queue, &waker, &events, "ses_1")
            .await
            .unwrap();
        assert_eq!(delivered, 2);
        assert_eq!(waker.calls().len(), 1);
        assert!(waker.calls()[0].contains("bg_1 exited"));
        assert!(waker.calls()[0].contains("go left"));
        assert!(queue
            .entries()
            .await
            .iter()
            .all(|entry| entry.state == SteerState::Delivered));
        let event = rx.try_recv().unwrap();
        assert_eq!(event.kind, EventKind::SteerDelivered);
        assert_eq!(event.data["route"], "sent at idle");
    }

    #[tokio::test]
    async fn failed_idle_wake_requeues() {
        let queue = SteerQueue::new();
        queue.enqueue(SteerKind::Steer, "retry").await;
        let events = EventBus::new(8);
        let waker = FakeWaker::new();
        waker.fail.store(true, Ordering::SeqCst);

        assert!(deliver_idle(&queue, &waker, &events, "ses_1")
            .await
            .is_err());
        assert_eq!(queue.pending_len().await, 1);
    }

    #[tokio::test]
    async fn delivery_is_decided_at_the_response_head() {
        let queue = SteerQueue::new();
        let entry = queue.enqueue(SteerKind::Steer, "head").await;
        let ids = vec![entry.id.clone()];
        queue.drain_pending().await;

        queue.response_head(&ids, 200).await;
        assert_eq!(queue.entries().await[0].state, SteerState::Delivered);
        queue.stream_failed_after_head(&ids).await;
        assert_eq!(queue.entries().await[0].state, SteerState::Delivered);
    }

    #[tokio::test]
    async fn non_2xx_and_connection_failure_requeue() {
        let queue = SteerQueue::new();
        let entry = queue.enqueue(SteerKind::Steer, "retry").await;
        let ids = vec![entry.id.clone()];

        queue.drain_pending().await;
        queue.response_head(&ids, 502).await;
        assert_eq!(queue.pending_len().await, 1);

        queue.drain_pending().await;
        queue.connection_failed(&ids).await;
        assert_eq!(queue.pending_len().await, 1);
    }

    #[tokio::test]
    async fn wake_waits_for_an_in_flight_request() {
        let queue = SteerQueue::new();
        queue.enqueue(SteerKind::Steer, "race").await;
        let events = EventBus::new(8);
        let waker = FakeWaker::new();

        queue.begin_request().await;
        assert_eq!(
            queue.wake_if_idle(&waker, &events, "ses_1").await.unwrap(),
            0
        );
        assert!(waker.calls().is_empty());

        queue.end_request().await;
        assert_eq!(
            queue.wake_if_idle(&waker, &events, "ses_1").await.unwrap(),
            1
        );
        assert_eq!(waker.calls().len(), 1);
    }

    #[tokio::test]
    async fn queue_survives_restart_and_requeues_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steer_queue.json");
        let queue = SteerQueue::open(&path).await;
        queue.enqueue(SteerKind::Steer, "persist me").await;
        queue.drain_pending().await;

        let restored = SteerQueue::open(&path).await;
        let entries = restored.entries().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "persist me");
        assert_eq!(entries[0].state, SteerState::Pending);
        assert_eq!(restored.pending_len().await, 1);
    }

    #[tokio::test]
    async fn idle_delivery_waits_while_a_turn_is_running() {
        let queue = SteerQueue::new();
        queue.enqueue(SteerKind::Steer, "mid-turn").await;
        let events = EventBus::new(8);
        let waker = FakeWaker::new();

        queue.set_turn_active(true);
        assert_eq!(
            deliver_idle(&queue, &waker, &events, "ses_1")
                .await
                .unwrap(),
            0
        );
        assert!(waker.calls().is_empty());
        assert_eq!(queue.pending_len().await, 1);

        queue.set_turn_active(false);
        assert_eq!(
            deliver_idle(&queue, &waker, &events, "ses_1")
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn idle_delivery_waits_for_an_in_flight_request() {
        let queue = SteerQueue::new();
        queue.enqueue(SteerKind::Steer, "race").await;
        let events = EventBus::new(8);
        let waker = FakeWaker::new();

        queue.begin_request().await;
        assert_eq!(
            deliver_idle(&queue, &waker, &events, "ses_1")
                .await
                .unwrap(),
            0
        );
        assert!(waker.calls().is_empty());
        assert_eq!(queue.pending_len().await, 1);
    }
}
