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
    /// A batched report waiting for its siblings or for the batch to seal.
    Held,
    InFlight,
    Delivered,
}

/// An image attached to a steer. Same wire shape as the harness `PromptImage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerImage {
    pub mime_type: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentReport {
    pub session_id: String,
    pub role: String,
    pub ticket_path: String,
    pub status: String,
    pub summary: String,
    pub report: String,
}

/// True when `role` identifies a persistent code-review Subagent (`spec` or `standards`).
pub fn is_reviewer_role(role: &str) -> bool {
    matches!(role, "spec" | "standards")
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerEntry {
    pub id: String,
    pub kind: SteerKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<SteerImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<SubagentReport>,
    pub state: SteerState,
    pub queued_at: u64,
    pub delivered_at: Option<u64>,
    pub route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<String>,
}

/// An open `spawn(batch=...)` barrier. Members register before their prompt is
/// dispatched; the barrier releases once it is sealed and no member is pending.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchState {
    pub name: String,
    pub sealed: bool,
    pub created_at: u64,
    pub deadline_at: u64,
    pub pending_ids: Vec<String>,
    pub completed_ids: Vec<String>,
}

pub const DEFAULT_BATCH_DEADLINE_MS: u64 = 30 * 60 * 1000;

#[derive(Default)]
struct QueueState {
    entries: Vec<SteerEntry>,
    batches: std::collections::BTreeMap<String, BatchState>,
}

/// A per-Session queue. `gate` serialises the turn-end wake with a proxy request
/// beginning, so a wake never overlaps an in-flight upstream request.
pub struct SteerQueue {
    state: AsyncMutex<QueueState>,
    gate: AsyncMutex<()>,
    in_flight: AtomicBool,
    turn_active: AtomicBool,
    path: Option<PathBuf>,
}

impl Default for SteerQueue {
    fn default() -> Self {
        Self {
            state: AsyncMutex::new(QueueState::default()),
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
        let state = load(&path);
        Self {
            state: AsyncMutex::new(state),
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
        self.enqueue_with_images(kind, text, Vec::new()).await
    }

    pub async fn enqueue_with_images(
        &self,
        kind: SteerKind,
        text: impl Into<String>,
        images: Vec<SteerImage>,
    ) -> SteerEntry {
        self.enqueue_with_id(generate_id("steer"), kind, text, images)
            .await
    }

    pub async fn enqueue_with_id(
        &self,
        id: impl Into<String>,
        kind: SteerKind,
        text: impl Into<String>,
        images: Vec<SteerImage>,
    ) -> SteerEntry {
        let entry = SteerEntry {
            id: id.into(),
            kind,
            text: text.into(),
            images,
            report: None,
            state: SteerState::Pending,
            queued_at: now_ms(),
            delivered_at: None,
            route: None,
            batch: None,
        };
        let mut state = self.state.lock().await;
        state.entries.push(entry.clone());
        self.save(&state);
        entry
    }

    /// Accept a report only after its queue entry is durable. A report for a
    /// batch member is held until the whole batch can be delivered together.
    pub async fn enqueue_report(&self, report: SubagentReport) -> std::io::Result<SteerEntry> {
        let header = if is_reviewer_role(&report.role) {
            format!(
                "Sub-agent report from {} reviewer ({}) for {} [{}]",
                report.role, report.session_id, report.ticket_path, report.status
            )
        } else if report.ticket_path.is_empty() {
            format!(
                "Sub-agent report from {} ({}) [{}]",
                report.role, report.session_id, report.status
            )
        } else {
            format!(
                "Sub-agent report from {} ({}) for {} [{}]",
                report.role, report.session_id, report.ticket_path, report.status
            )
        };
        let mut state = self.state.lock().await;
        let batch = state
            .batches
            .values()
            .find(|batch| batch.pending_ids.contains(&report.session_id))
            .map(|batch| batch.name.clone());
        let entry = SteerEntry {
            id: generate_id("submit"),
            kind: SteerKind::Steer,
            text: format!("{header}\n{}\n\n{}", report.summary, report.report),
            images: Vec::new(),
            report: Some(report),
            state: if batch.is_some() {
                SteerState::Held
            } else {
                SteerState::Pending
            },
            queued_at: now_ms(),
            delivered_at: None,
            route: None,
            batch,
        };
        state.entries.push(entry.clone());
        if let Err(error) = self.try_save(&state) {
            state.entries.pop();
            return Err(error);
        }
        Ok(entry)
    }

    /// Record a member before its prompt is dispatched. A batch that starts
    /// while no turn is active is already sealed and releases when its last
    /// member finishes.
    pub async fn register_batch_member(
        &self,
        batch: &str,
        child_id: &str,
        deadline_ms: u64,
    ) -> Result<(), String> {
        let mut state = self.state.lock().await;
        if let Some(existing) = state.batches.get(batch) {
            if existing.sealed {
                return Err(format!("batch `{batch}` is sealed"));
            }
        }
        let sealed = !self.turn_active.load(Ordering::SeqCst);
        if state
            .batches
            .values()
            .any(|open| open.pending_ids.iter().any(|id| id == child_id))
        {
            return Err(format!("{child_id} is already pending in a batch"));
        }
        let now = now_ms();
        let entry = state
            .batches
            .entry(batch.to_string())
            .or_insert(BatchState {
                name: batch.to_string(),
                sealed,
                created_at: now,
                deadline_at: now.saturating_add(deadline_ms),
                pending_ids: Vec::new(),
                completed_ids: Vec::new(),
            });
        entry.pending_ids.push(child_id.to_string());
        if let Err(error) = self.try_save(&state) {
            if let Some(open) = state.batches.get_mut(batch) {
                open.pending_ids.retain(|id| id != child_id);
                if open.pending_ids.is_empty() && open.completed_ids.is_empty() {
                    state.batches.remove(batch);
                }
            }
            return Err(error.to_string());
        }
        Ok(())
    }

    /// Drop a member whose prompt never reached dispatch. Returns entries
    /// released because this was the last pending member of a sealed batch.
    pub async fn unregister_batch_member(&self, child_id: &str) -> Vec<SteerEntry> {
        let mut state = self.state.lock().await;
        let Some(name) = state
            .batches
            .values()
            .find(|batch| batch.pending_ids.iter().any(|id| id == child_id))
            .map(|batch| batch.name.clone())
        else {
            return Vec::new();
        };
        if let Some(batch) = state.batches.get_mut(&name) {
            batch.pending_ids.retain(|id| id != child_id);
        }
        let empty = state
            .batches
            .get(&name)
            .is_some_and(|batch| batch.pending_ids.is_empty() && batch.completed_ids.is_empty());
        if empty {
            state.batches.remove(&name);
        }
        let released = release_ready(&mut state);
        self.save(&state);
        released
    }

    /// Mark one member finished and release its sealed batch when no sibling is
    /// still pending. Returns the entries released by that completion.
    pub async fn complete_batch_member(&self, child_id: &str) -> Vec<SteerEntry> {
        let mut state = self.state.lock().await;
        let Some(name) = state
            .batches
            .values()
            .find(|batch| batch.pending_ids.iter().any(|id| id == child_id))
            .map(|batch| batch.name.clone())
        else {
            return Vec::new();
        };
        if let Some(batch) = state.batches.get_mut(&name) {
            batch.pending_ids.retain(|id| id != child_id);
            batch.completed_ids.push(child_id.to_string());
        }
        let released = release_ready(&mut state);
        self.save(&state);
        released
    }

    /// Release every batch past its deadline with a synthetic `timed_out` report
    /// for each member that never reported.
    pub async fn expire_batches(&self, now: u64) -> Vec<SteerEntry> {
        let mut state = self.state.lock().await;
        let overdue: Vec<(String, Vec<String>)> = state
            .batches
            .values()
            .filter(|batch| now >= batch.deadline_at)
            .map(|batch| (batch.name.clone(), batch.pending_ids.clone()))
            .collect();
        let mut released = Vec::new();
        for (name, pending) in overdue {
            let warning = SteerEntry {
                id: generate_id("batch"),
                kind: SteerKind::Notice,
                text: format!(
                    "Batch \"{name}\" timed out: {} member(s) did not report.",
                    pending.len()
                ),
                images: Vec::new(),
                report: None,
                state: SteerState::Pending,
                queued_at: now_ms(),
                delivered_at: None,
                route: None,
                batch: None,
            };
            insert_before_batch(&mut state, &name, warning.clone());
            released.push(warning);
            for child in &pending {
                state.entries.push(synthetic_entry(
                    &name,
                    child,
                    "timed_out",
                    "Batch member timed out.",
                    "No report arrived before the batch deadline.",
                ));
            }
            if let Some(batch) = state.batches.get_mut(&name) {
                batch.sealed = true;
                batch.completed_ids.append(&mut batch.pending_ids);
            }
            released.extend(release_batch(&mut state, &name));
        }
        self.save(&state);
        released
    }

    /// Seal batches left open by a previous daemon and close members whose
    /// process-local run was lost. Returns the entries released by that seal.
    pub async fn reconcile_on_startup(&self) -> Vec<SteerEntry> {
        let mut state = self.state.lock().await;
        if state.batches.is_empty() {
            return Vec::new();
        }
        for batch in state.batches.values_mut() {
            batch.sealed = true;
        }
        if !self.turn_active.load(Ordering::SeqCst) {
            let pending: Vec<(String, Vec<String>)> = state
                .batches
                .values()
                .map(|batch| (batch.name.clone(), batch.pending_ids.clone()))
                .collect();
            for (name, children) in pending {
                for child in &children {
                    state.entries.push(synthetic_entry(
                        &name,
                        child,
                        "errored",
                        "Batch member lost on daemon restart.",
                        "Open the subagent Session to inspect its work. No automatic retry.",
                    ));
                }
                if let Some(batch) = state.batches.get_mut(&name) {
                    batch.completed_ids.append(&mut batch.pending_ids);
                }
            }
        }
        let released = release_ready(&mut state);
        self.save(&state);
        released
    }

    pub async fn has_open_batch(&self) -> bool {
        !self.state.lock().await.batches.is_empty()
    }

    /// Mark every pending entry in flight before one upstream request carries
    /// the whole queue as a single prompt.
    pub async fn drain_pending(&self) -> Vec<SteerEntry> {
        let mut state = self.state.lock().await;
        let mut drained = Vec::new();
        for entry in state.entries.iter_mut() {
            if entry.state == SteerState::Pending {
                entry.state = SteerState::InFlight;
                drained.push(entry.clone());
            }
        }
        self.save(&state);
        drained
    }

    pub async fn mark_in_flight(&self, ids: &[String]) {
        let mut state = self.state.lock().await;
        for entry in state.entries.iter_mut() {
            if ids.contains(&entry.id) {
                entry.state = SteerState::InFlight;
            }
        }
        self.save(&state);
    }

    pub async fn mark_delivered(&self, ids: &[String], route: &str) {
        let now = now_ms();
        let mut state = self.state.lock().await;
        for entry in state.entries.iter_mut() {
            if ids.contains(&entry.id) {
                entry.state = SteerState::Delivered;
                entry.delivered_at = Some(now);
                entry.route = Some(route.to_string());
            }
        }
        self.save(&state);
    }

    pub async fn requeue(&self, ids: &[String]) {
        let mut state = self.state.lock().await;
        for entry in state.entries.iter_mut() {
            if ids.contains(&entry.id) {
                entry.state = SteerState::Pending;
                entry.delivered_at = None;
                entry.route = None;
            }
        }
        self.save(&state);
    }

    /// A request owns the Session until `end_request`. Delivery is decided at
    /// the response head.
    pub async fn begin_request(&self) {
        let _gate = self.gate.lock().await;
        self.in_flight.store(true, Ordering::SeqCst);
    }

    /// A turn is running from its prompt until the harness reports it over; a
    /// steer queued meanwhile waits for the proxy to inject it. Ending the turn
    /// seals every open batch. Returns the entries released as the turn ends.
    pub async fn set_turn_active(&self, active: bool) -> Vec<SteerEntry> {
        let mut state = self.state.lock().await;
        self.turn_active.store(active, Ordering::SeqCst);
        if active {
            return Vec::new();
        }
        for batch in state.batches.values_mut() {
            batch.sealed = true;
        }
        let released = release_ready(&mut state);
        self.save(&state);
        released
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
        let images: Vec<SteerImage> = entries
            .iter()
            .flat_map(|entry| entry.images.clone())
            .collect();
        if let Err(error) = waker.wake(&text, &images).await {
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
        self.state
            .lock()
            .await
            .entries
            .iter()
            .filter(|entry| entry.state == SteerState::Pending)
            .count()
    }

    pub async fn entries(&self) -> Vec<SteerEntry> {
        self.state.lock().await.entries.clone()
    }

    fn save(&self, state: &QueueState) {
        let _ = self.try_save(state);
    }

    fn try_save(&self, state: &QueueState) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = if state.batches.is_empty() {
            serde_json::to_vec_pretty(&state.entries)?
        } else {
            serde_json::to_vec_pretty(&StoredQueue::Full {
                entries: state.entries.clone(),
                batches: state.batches.clone(),
            })?
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

/// Sealed batches with no pending member release together, each behind a notice.
fn release_ready(state: &mut QueueState) -> Vec<SteerEntry> {
    let ready: Vec<String> = state
        .batches
        .values()
        .filter(|batch| batch.sealed && batch.pending_ids.is_empty())
        .map(|batch| batch.name.clone())
        .collect();
    ready
        .iter()
        .flat_map(|name| release_batch(state, name))
        .collect()
}

fn release_batch(state: &mut QueueState, name: &str) -> Vec<SteerEntry> {
    let Some(batch) = state.batches.remove(name) else {
        return Vec::new();
    };
    let ids: Vec<String> = state
        .entries
        .iter()
        .filter(|entry| entry.batch.as_deref() == Some(name) && entry.state == SteerState::Held)
        .map(|entry| entry.id.clone())
        .collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let total = batch.completed_ids.len() + batch.pending_ids.len();
    let notice = SteerEntry {
        id: generate_id("batch"),
        kind: SteerKind::Notice,
        text: format!(
            "Batch \"{name}\" completed ({}/{}):",
            batch.completed_ids.len(),
            total
        ),
        images: Vec::new(),
        report: None,
        state: SteerState::Pending,
        queued_at: now_ms(),
        delivered_at: None,
        route: None,
        batch: None,
    };
    let at = state
        .entries
        .iter()
        .position(|entry| entry.batch.as_deref() == Some(name))
        .unwrap_or(state.entries.len());
    state.entries.insert(at, notice.clone());
    let mut released = vec![notice];
    for entry in state.entries.iter_mut() {
        if ids.contains(&entry.id) {
            entry.state = SteerState::Pending;
            released.push(entry.clone());
        }
    }
    released
}

fn insert_before_batch(state: &mut QueueState, name: &str, entry: SteerEntry) {
    let at = state
        .entries
        .iter()
        .position(|entry| entry.batch.as_deref() == Some(name))
        .unwrap_or(state.entries.len());
    state.entries.insert(at, entry);
}

fn synthetic_entry(
    batch: &str,
    child_id: &str,
    status: &str,
    summary: &str,
    body: &str,
) -> SteerEntry {
    let report = SubagentReport {
        session_id: child_id.to_string(),
        role: "worker".to_string(),
        ticket_path: String::new(),
        status: status.to_string(),
        summary: summary.to_string(),
        report: body.to_string(),
    };
    SteerEntry {
        id: generate_id("submit"),
        kind: SteerKind::Steer,
        text: format!("Sub-agent report from worker ({child_id}) [{status}]\n{summary}\n\n{body}"),
        images: Vec::new(),
        report: Some(report),
        state: SteerState::Held,
        queued_at: now_ms(),
        delivered_at: None,
        route: None,
        batch: Some(batch.to_string()),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredQueue {
    Bare(Vec<SteerEntry>),
    Full {
        #[serde(default)]
        entries: Vec<SteerEntry>,
        #[serde(default)]
        batches: std::collections::BTreeMap<String, BatchState>,
    },
}

fn load(path: &Path) -> QueueState {
    let mut state = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<StoredQueue>(&text).ok())
        .map(|stored| match stored {
            StoredQueue::Bare(entries) => QueueState {
                entries,
                batches: Default::default(),
            },
            StoredQueue::Full { entries, batches } => QueueState { entries, batches },
        })
        .unwrap_or_default();
    for entry in &mut state.entries {
        if entry.state == SteerState::InFlight {
            entry.state = SteerState::Pending;
        }
    }
    state
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
                        state: AsyncMutex::new(load(&path)),
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
    async fn wake(&self, text: &str, images: &[SteerImage]) -> Result<(), String>;
}

/// Route 2: no turn is running, so wake the harness once with every pending entry as a
/// single prompt. While a turn runs the proxy injects the entries on its next request
/// (route 1), so this returns 0. A failed wake puts the entries back.
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
    let images: Vec<SteerImage> = entries
        .iter()
        .flat_map(|entry| entry.images.clone())
        .collect();
    if let Err(error) = waker.wake(&text, &images).await {
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
        images: Mutex<Vec<SteerImage>>,
    }

    impl FakeWaker {
        fn new() -> Self {
            Self {
                fail: AtomicBool::new(false),
                calls: Mutex::new(Vec::new()),
                images: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SteerWaker for FakeWaker {
        async fn wake(&self, text: &str, images: &[SteerImage]) -> Result<(), String> {
            self.calls.lock().unwrap().push(text.to_string());
            self.images.lock().unwrap().extend_from_slice(images);
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
    async fn idle_wake_carries_steer_images() {
        let queue = SteerQueue::new();
        let image = SteerImage {
            mime_type: "image/png".into(),
            data_base64: "AAAA".into(),
        };
        queue
            .enqueue_with_images(SteerKind::Steer, "look", vec![image.clone()])
            .await;
        let waker = FakeWaker::new();

        deliver_idle(&queue, &waker, &EventBus::new(8), "ses_1")
            .await
            .unwrap();
        assert_eq!(*waker.images.lock().unwrap(), vec![image]);
    }

    #[tokio::test]
    async fn a_steer_during_a_turn_waits_for_the_proxy() {
        let queue = SteerQueue::new();
        queue.enqueue(SteerKind::Steer, "go left").await;
        queue.set_turn_active(true).await;
        let events = EventBus::new(8);
        let waker = FakeWaker::new();

        let held = deliver_idle(&queue, &waker, &events, "ses_1")
            .await
            .unwrap();
        assert_eq!(held, 0, "the proxy carries a steer while a turn runs");
        assert!(waker.calls().is_empty());
        assert_eq!(queue.pending_len().await, 1);
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

        queue.set_turn_active(true).await;
        assert_eq!(
            deliver_idle(&queue, &waker, &events, "ses_1")
                .await
                .unwrap(),
            0
        );
        assert!(waker.calls().is_empty());
        assert_eq!(queue.pending_len().await, 1);

        queue.set_turn_active(false).await;
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

    #[tokio::test]
    async fn set_turn_active_is_async_and_releases_nothing_yet() {
        let queue = SteerQueue::new();
        assert!(queue.set_turn_active(false).await.is_empty());
        assert!(!queue.is_turn_active());
    }

    #[tokio::test]
    async fn steer_queued_event_carries_the_same_report_payload() {
        let queue = SteerQueue::new();
        let report = SubagentReport {
            session_id: "ses_child".into(),
            role: "worker".into(),
            ticket_path: "issues/01.md".into(),
            status: "done".into(),
            summary: "one line".into(),
            report: "body".into(),
        };
        let entry = queue.enqueue_report(report.clone()).await.unwrap();
        let event = SessionEvent::steer_queued("ses_parent", &entry);
        assert_eq!(event.session_id, "ses_parent");
        assert_eq!(event.seq, 0);
        assert_eq!(event.kind, EventKind::SteerQueued);
        assert_eq!(event.data["id"], entry.id);
        assert_eq!(event.data["text"], entry.text);
        assert_eq!(event.data["kind"], "subagent");
        assert_eq!(event.data["report"], serde_json::json!(report));
        assert_eq!(event.data["imagesCount"], 0);
    }

    #[tokio::test]
    async fn steer_queued_event_keeps_the_plain_steer_shape() {
        let queue = SteerQueue::new();
        let entry = queue.enqueue(SteerKind::Steer, "go left").await;
        let event = SessionEvent::steer_queued("ses_1", &entry);
        assert_eq!(event.data["kind"], "steer");
        assert_eq!(event.data["imagesCount"], 0);
        assert!(event.data.get("report").is_none());
    }

    #[tokio::test]
    async fn steer_queued_event_names_a_notice() {
        let queue = SteerQueue::new();
        let entry = queue.enqueue(SteerKind::Notice, "bg_1 exited").await;
        assert_eq!(
            SessionEvent::steer_queued("ses_1", &entry).data["kind"],
            "notice"
        );
    }

    fn report(child: &str, summary: &str) -> SubagentReport {
        SubagentReport {
            session_id: child.into(),
            role: "worker".into(),
            ticket_path: String::new(),
            status: "done".into(),
            summary: summary.into(),
            report: format!("report from {child}"),
        }
    }

    #[tokio::test]
    async fn held_batch_reports_release_together_behind_one_notice() {
        let queue = SteerQueue::new();
        queue.set_turn_active(true).await;
        queue
            .register_batch_member("b1", "child1", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap();
        queue
            .register_batch_member("b1", "child2", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap();
        queue
            .enqueue_report(report("child1", "first"))
            .await
            .unwrap();
        queue
            .enqueue_report(report("child2", "second"))
            .await
            .unwrap();

        assert_eq!(queue.pending_len().await, 0);
        assert!(queue.drain_pending().await.is_empty());
        assert!(queue.has_open_batch().await);

        assert!(queue.set_turn_active(false).await.is_empty());
        assert!(queue.complete_batch_member("child1").await.is_empty());
        let released = queue.complete_batch_member("child2").await;

        assert_eq!(released.len(), 3);
        assert_eq!(released[0].text, "Batch \"b1\" completed (2/2):");
        assert_eq!(released[1].report.as_ref().unwrap().session_id, "child1");
        assert_eq!(released[2].report.as_ref().unwrap().session_id, "child2");
        assert_eq!(queue.pending_len().await, 3);
        assert!(!queue.has_open_batch().await);
    }

    #[tokio::test]
    async fn batch_registration_rejects_a_sealed_batch_or_a_duplicate_member() {
        let queue = SteerQueue::new();
        queue.set_turn_active(true).await;
        queue
            .register_batch_member("b1", "child1", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap();
        assert!(queue
            .register_batch_member("b1", "child1", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap_err()
            .contains("already pending"));

        queue.set_turn_active(false).await;
        assert!(queue
            .register_batch_member("b1", "child2", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap_err()
            .contains("sealed"));
    }

    #[tokio::test]
    async fn batch_queue_persists_batches_and_keeps_held_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steer_queue.json");
        let queue = SteerQueue::open(&path).await;
        queue.set_turn_active(true).await;
        queue
            .register_batch_member("b1", "child1", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap();
        queue
            .enqueue_report(report("child1", "held"))
            .await
            .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"batches\""), "{text}");
        let restored = SteerQueue::open(&path).await;
        assert_eq!(restored.entries().await[0].state, SteerState::Held);
        assert!(restored.has_open_batch().await);
    }

    #[tokio::test]
    async fn queue_without_batches_persists_a_bare_array() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steer_queue.json");
        let queue = SteerQueue::open(&path).await;
        queue.enqueue(SteerKind::Steer, "plain").await;
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .trim_start()
            .starts_with('['));
    }

    #[tokio::test]
    async fn unregister_drops_an_empty_batch_and_releases_a_sealed_one() {
        let empty = SteerQueue::new();
        empty.set_turn_active(true).await;
        empty
            .register_batch_member("brief", "child1", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap();
        assert!(empty.unregister_batch_member("child1").await.is_empty());
        assert!(!empty.has_open_batch().await);

        let queue = SteerQueue::new();
        queue.set_turn_active(true).await;
        queue
            .register_batch_member("b1", "child1", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap();
        queue
            .enqueue_report(report("child1", "held"))
            .await
            .unwrap();
        queue.complete_batch_member("child1").await;
        queue
            .register_batch_member("b1", "child2", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap();
        queue.set_turn_active(false).await;
        let released = queue.unregister_batch_member("child2").await;
        assert_eq!(released.len(), 2);
        assert_eq!(released[0].text, "Batch \"b1\" completed (1/1):");
        assert_eq!(released[1].report.as_ref().unwrap().session_id, "child1");
        assert_eq!(queue.pending_len().await, 2);
        assert!(!queue.has_open_batch().await);
    }

    #[tokio::test]
    async fn expired_batch_times_out_pending_members_and_releases_everything() {
        let queue = SteerQueue::new();
        queue.set_turn_active(true).await;
        queue
            .register_batch_member("b1", "child1", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap();
        queue
            .register_batch_member("b1", "child2", DEFAULT_BATCH_DEADLINE_MS)
            .await
            .unwrap();
        queue
            .enqueue_report(report("child1", "finished"))
            .await
            .unwrap();
        queue.complete_batch_member("child1").await;
        assert!(queue.is_turn_active());
        assert!(queue.has_open_batch().await);
        assert!(queue.expire_batches(0).await.is_empty());

        let released = queue.expire_batches(u64::MAX).await;

        assert_eq!(released.len(), 4);
        assert!(
            released[0].text.contains("timed out"),
            "{}",
            released[0].text
        );
        assert_eq!(released[1].text, "Batch \"b1\" completed (2/2):");
        assert_eq!(released[2].report.as_ref().unwrap().status, "done");
        assert_eq!(released[3].report.as_ref().unwrap().session_id, "child2");
        assert_eq!(released[3].report.as_ref().unwrap().status, "timed_out");
        assert_eq!(queue.pending_len().await, 4);
        assert!(!queue.has_open_batch().await);
    }

    #[tokio::test]
    async fn startup_reconcile_closes_orphans_and_releases_unsealed_or_sealed_batches() {
        for sealed_before_restart in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("steer_queue.json");
            {
                let queue = SteerQueue::open(&path).await;
                queue.set_turn_active(true).await;
                queue
                    .register_batch_member("b1", "child1", DEFAULT_BATCH_DEADLINE_MS)
                    .await
                    .unwrap();
                queue
                    .register_batch_member("b1", "child2", DEFAULT_BATCH_DEADLINE_MS)
                    .await
                    .unwrap();
                queue
                    .enqueue_report(report("child1", "finished"))
                    .await
                    .unwrap();
                queue.complete_batch_member("child1").await;
                if sealed_before_restart {
                    queue.set_turn_active(false).await;
                }
            }

            let restored = SteerQueue::open(&path).await;
            assert!(restored.has_open_batch().await);
            let released = restored.reconcile_on_startup().await;

            assert_eq!(released.len(), 3, "sealed={sealed_before_restart}");
            assert_eq!(released[0].text, "Batch \"b1\" completed (2/2):");
            assert_eq!(released[1].report.as_ref().unwrap().status, "done");
            assert_eq!(released[2].report.as_ref().unwrap().session_id, "child2");
            assert_eq!(released[2].report.as_ref().unwrap().status, "errored");
            assert!(!restored.has_open_batch().await);
        }
    }
}
