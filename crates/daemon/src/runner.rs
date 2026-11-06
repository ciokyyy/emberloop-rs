use std::collections::HashMap;
use std::sync::Arc;

use crate::tooldisplay::ToolTracker;
use api::driver::SessionDriver;
use api::routes::PromptImageDto;
use emberloop_core::events::{EventBus, EventKind, SessionEvent};
use emberloop_core::steer::{deliver_idle, SteerKind, SteerQueue, SteerQueues, SteerWaker};
use futures::StreamExt;
use harness::{Harness, HarnessEvent, HarnessSession, HarnessWiring, PromptImage};
use proxy::ProxyState;
use store::{DataRoot, EventLog, ProjectsStore, SessionMeta, SessionsStore};
use tokio::sync::{Mutex, RwLock};

/// The daemon's turn loop: one harness process per Session, its event stream
/// folded into the Session log, and idle steer delivery at turn end.
pub struct SessionRunner {
    harness: Arc<dyn Harness>,
    sessions: SessionsStore,
    projects: ProjectsStore,
    data_root: DataRoot,
    events: Arc<EventBus>,
    steers: SteerQueues,
    proxy: ProxyState,
    proxy_base: String,
    logs: Arc<RwLock<HashMap<String, EventLog>>>,
    account_home: Option<std::path::PathBuf>,
    handles: Arc<Mutex<HashMap<String, Arc<Handle>>>>,
}

fn log_failure(error: Option<impl std::fmt::Display>, what: &str) {
    if let Some(error) = error {
        tracing::warn!(%error, "{what}");
    }
}

struct Handle {
    session: Arc<dyn HarnessSession>,
    queue: Arc<SteerQueue>,
}

impl SessionRunner {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        harness: Arc<dyn Harness>,
        sessions: SessionsStore,
        projects: ProjectsStore,
        data_root: DataRoot,
        events: Arc<EventBus>,
        steers: SteerQueues,
        proxy: ProxyState,
        proxy_base: String,
        logs: Arc<RwLock<HashMap<String, EventLog>>>,
        account_home: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            harness,
            sessions,
            projects,
            data_root,
            events,
            steers,
            proxy,
            proxy_base,
            logs,
            account_home,
            handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn wiring(
        &self,
        session_id: &str,
        root: &std::path::Path,
        meta: &SessionMeta,
    ) -> HarnessWiring {
        let spill = self.data_root.session_spill_dir(session_id);
        let mcp_command = crate::daemon_exe().map(|exe| {
            vec![
                exe.display().to_string(),
                "mcp".to_string(),
                "--root".to_string(),
                root.display().to_string(),
                "--spill".to_string(),
                spill.display().to_string(),
            ]
        });
        HarnessWiring {
            proxy_url: format!(
                "{}/proxy/{}",
                self.proxy_base.trim_end_matches('/'),
                self.proxy.token_for(session_id)
            ),
            mcp_url: None,
            mcp_command,
            env: Vec::new(),
            model: Some(meta.model.clone()),
            thought_level: meta.thought_level.clone(),
            resume_token: meta.resume_token.clone(),
            workdir: Some(self.data_root.session_dir(session_id).join("agent-work")),
        }
    }

    /// Remember the harness's id for the conversation, and drop the proxy's
    /// stored history when that conversation started over instead of loading.
    async fn settle_conversation(
        &self,
        session_id: &str,
        meta: &SessionMeta,
        session: &dyn HarnessSession,
    ) {
        if !session.resumed() {
            self.reset_proxy_history(session_id).await;
        }
        let token = session.resume_token();
        if token != meta.resume_token {
            let mut meta = meta.clone();
            meta.resume_token = token;
            self.store_meta(&meta).await;
        }
    }

    async fn reset_proxy_history(&self, session_id: &str) {
        let reset = self.proxy.reset_history(session_id).await;
        log_failure(reset.err(), "could not reset the proxy history");
    }

    async fn store_meta(&self, meta: &SessionMeta) {
        let saved = self.sessions.save(meta).await;
        log_failure(saved.err(), "could not store the resume token");
    }

    async fn ensure(&self, session_id: &str) -> Result<Arc<Handle>, String> {
        if let Some(handle) = self.handles.lock().await.get(session_id).cloned() {
            return Ok(handle);
        }
        let meta = self
            .sessions
            .get(session_id)
            .await
            .ok_or_else(|| "Session not found".to_string())?;
        let project = self
            .projects
            .get(&meta.project_id)
            .await
            .ok_or_else(|| "Project not found".to_string())?;
        let wiring = self.wiring(session_id, &project.root, &meta);
        let session = Arc::<dyn HarnessSession>::from(
            self.harness
                .spawn(session_id, wiring)
                .await
                .map_err(|error| error.to_string())?,
        );
        self.settle_conversation(session_id, &meta, session.as_ref())
            .await;
        let handle = Arc::new(Handle {
            session,
            queue: self.steers.queue(session_id),
        });

        let mut handles = self.handles.lock().await;
        if let Some(existing) = handles.get(session_id) {
            return Ok(existing.clone());
        }
        handles.insert(session_id.to_string(), handle.clone());
        drop(handles);

        tokio::spawn(pump(
            handle.clone(),
            session_id.to_string(),
            Arc::clone(&self.events),
            Arc::clone(&self.logs),
            self.data_root.session_events_dir(session_id),
            self.data_root.clone(),
            self.steers.clone(),
            project.root.clone(),
            Arc::clone(&self.handles),
        ));
        Ok(handle)
    }

    /// Notices the tool process wrote while the daemon was down, or after a
    /// background exit it could not report itself.
    async fn drain_notices(&self, session_id: &str) {
        drain_notices(&self.data_root, &self.steers, session_id).await;
    }

    async fn handle(&self, session_id: &str) -> Option<Arc<Handle>> {
        self.handles.lock().await.get(session_id).cloned()
    }
}

#[async_trait::async_trait]
impl SessionDriver for SessionRunner {
    async fn prompt(
        &self,
        session_id: &str,
        text: &str,
        images: Vec<PromptImageDto>,
    ) -> Result<(), String> {
        let is_pi = self
            .sessions
            .get(session_id)
            .await
            .is_some_and(|meta| crate::is_pi_model(Some(&meta.model)));
        if !is_pi {
            if let Some(home) = &self.account_home {
                if !home.join("antigravity-acp/acp_token.json").exists() {
                    let message = "no Antigravity account is signed in; open Providers to sign in";
                    let event = SessionEvent::new(
                        session_id,
                        0,
                        EventKind::Error,
                        serde_json::json!({ "message": message }),
                    );
                    self.events.publish(event);
                    return Err(message.to_string());
                }
            }
        }
        self.drain_notices(session_id).await;
        let handle = self.ensure(session_id).await?;
        let images: Vec<PromptImage> = images
            .into_iter()
            .map(|image| PromptImage {
                mime_type: image.mime_type,
                data_base64: image.data_base64,
            })
            .collect();
        handle.queue.set_turn_active(true);
        let sent = handle.session.send(text, images).await;
        if sent.is_err() {
            handle.queue.set_turn_active(false);
        }
        sent.map_err(|error| error.to_string())
    }

    async fn cancel(&self, session_id: &str) -> Result<(), String> {
        match self.handle(session_id).await {
            Some(handle) => handle
                .session
                .cancel()
                .await
                .map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    async fn wake(&self, session_id: &str, text: &str) -> Result<(), String> {
        let handle = self.ensure(session_id).await?;
        handle.queue.set_turn_active(true);
        let woken = handle.session.wake(text).await;
        if woken.is_err() {
            handle.queue.set_turn_active(false);
        } else {
            let status_ev = SessionEvent::new(
                session_id,
                0,
                EventKind::Status,
                serde_json::json!({ "state": "working" }),
            );
            let events_dir = self.data_root.session_events_dir(session_id);
            if let Ok(stored) = append(&self.logs, &events_dir, &status_ev).await {
                self.events.publish(stored);
            }
        }
        woken.map_err(|error| error.to_string())
    }

    async fn connect(&self, session_id: &str) -> Result<(), String> {
        self.ensure(session_id).await.map(|_| ())
    }

    async fn set_thought_level(&self, session_id: &str, level: &str) -> Result<(), String> {
        if let Some(handle) = self.handle(session_id).await {
            handle
                .session
                .set_thought_level(level)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn pump(
    handle: Arc<Handle>,
    session_id: String,
    events: Arc<EventBus>,
    logs: Arc<RwLock<HashMap<String, EventLog>>>,
    events_dir: std::path::PathBuf,
    data_root: DataRoot,
    steers: SteerQueues,
    project_root: std::path::PathBuf,
    handles: Arc<Mutex<HashMap<String, Arc<Handle>>>>,
) {
    let mut stream = handle.session.events();
    let mut tracker = ToolTracker::default();
    while let Some(event) = stream.next().await {
        let turn_over = matches!(event, HarnessEvent::TurnFinished { .. });
        if turn_over || matches!(event, HarnessEvent::Error(_)) {
            handle.queue.set_turn_active(false);
        }
        if let Some(mapped) = map_event(&session_id, event, &mut tracker, &project_root) {
            let stored = match append(&logs, &events_dir, &mapped).await {
                Ok(stored) => stored,
                Err(error) => {
                    let failure = SessionEvent::new(
                        &session_id,
                        0,
                        EventKind::Error,
                        serde_json::json!({ "message": format!("event log error: {error}") }),
                    );
                    append(&logs, &events_dir, &failure)
                        .await
                        .unwrap_or(failure)
                }
            };
            events.publish(stored);
        }
        if turn_over {
            drain_notices(&data_root, &steers, &session_id).await;
            apply_pending_update(&data_root, &handles, &events, &session_id).await;
            let waker = SessionWaker {
                handle: Arc::clone(&handle),
                session_id: session_id.clone(),
                events: Arc::clone(&events),
                logs: Arc::clone(&logs),
                events_dir: events_dir.clone(),
            };
            let _ = deliver_idle(&handle.queue, &waker, &events, &session_id).await;
        }
    }
}

struct SessionWaker {
    handle: Arc<Handle>,
    session_id: String,
    events: Arc<EventBus>,
    logs: Arc<RwLock<HashMap<String, EventLog>>>,
    events_dir: std::path::PathBuf,
}

#[async_trait::async_trait]
impl SteerWaker for SessionWaker {
    async fn wake(&self, text: &str) -> Result<(), String> {
        self.handle.queue.set_turn_active(true);
        let res = self
            .handle
            .session
            .wake(text)
            .await
            .map_err(|error| error.to_string());
        if res.is_err() {
            self.handle.queue.set_turn_active(false);
        } else {
            let status_ev = SessionEvent::new(
                &self.session_id,
                0,
                EventKind::Status,
                serde_json::json!({ "state": "working" }),
            );
            if let Ok(stored) = append(&self.logs, &self.events_dir, &status_ev).await {
                self.events.publish(stored);
            }
        }
        res
    }
}

fn map_event(
    session_id: &str,
    event: HarnessEvent,
    tracker: &mut ToolTracker,
    project_root: &std::path::Path,
) -> Option<SessionEvent> {
    let (kind, data) = match event {
        HarnessEvent::AssistantChunk(text) => (
            EventKind::AssistantChunk,
            serde_json::json!({ "text": text }),
        ),
        HarnessEvent::ThoughtChunk(text) => (
            EventKind::ThinkingChunk,
            serde_json::json!({ "text": text }),
        ),
        HarnessEvent::ToolStarted { id, name, input } => {
            let display = tracker.on_start(&id, &name, &input, project_root);
            let mut obj = serde_json::json!({ "id": id, "name": name, "input": input });
            if let Some(display) = display {
                obj["display"] = display;
            }
            (EventKind::ToolStarted, obj)
        }
        HarnessEvent::ToolFinished { id, output, status } => {
            let (name, input, display) = tracker.on_finish(&id, project_root);
            let mut obj = serde_json::json!({
                "id": id,
                "output": output,
                "status": status.unwrap_or_else(|| "completed".to_string()),
            });
            if let Some(name) = name {
                obj["name"] = serde_json::Value::String(name);
            }
            if let Some(input) = input {
                obj["input"] = input;
            }
            if let Some(display) = display {
                obj["display"] = display;
            }
            (EventKind::ToolFinished, obj)
        }
        HarnessEvent::TurnFinished { stop_reason, usage } => (
            EventKind::Status,
            serde_json::json!({ "state": "idle", "stopReason": stop_reason, "usage": usage }),
        ),
        HarnessEvent::Error(message) => {
            (EventKind::Error, serde_json::json!({ "message": message }))
        }
    };
    Some(SessionEvent::new(session_id, 0, kind, data))
}

async fn append(
    logs: &Arc<RwLock<HashMap<String, EventLog>>>,
    events_dir: &std::path::Path,
    event: &SessionEvent,
) -> Result<SessionEvent, String> {
    let log = {
        let mut logs = logs.write().await;
        match logs.get(&event.session_id) {
            Some(log) => log.clone(),
            None => {
                let log = EventLog::open(events_dir)
                    .await
                    .map_err(|error| error.to_string())?;
                logs.insert(event.session_id.clone(), log.clone());
                log
            }
        }
    };
    log.append(event.clone())
        .await
        .map_err(|error| error.to_string())
}

async fn drain_notices(data_root: &DataRoot, steers: &SteerQueues, session_id: &str) {
    let path = data_root.session_dir(session_id).join("notices.jsonl");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    if std::fs::write(&path, "").is_err() {
        return;
    }
    let queue = steers.queue(session_id);
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(message) = value.get("text").and_then(|text| text.as_str()) {
            queue.enqueue(SteerKind::Notice, message).await;
        }
    }
}

/// Every turn end checks for an update a model asked for. It runs only when no Session has a turn
/// running, so the restart never cuts a model off; the last Session to go idle triggers it.
async fn apply_pending_update(
    data_root: &DataRoot,
    handles: &Mutex<HashMap<String, Arc<Handle>>>,
    events: &EventBus,
    session_id: &str,
) {
    if crate::update::read_request(data_root.root()).is_none() {
        return;
    }
    if handles
        .lock()
        .await
        .values()
        .any(|handle| handle.queue.is_turn_active())
    {
        return;
    }
    let root = crate::update::install_root();
    match crate::update::apply_request(&root, data_root.root()).await {
        Ok(Some(_)) => restart_into(&root),
        Ok(None) => {}
        Err(error) => events.publish(SessionEvent::new(
            session_id,
            0,
            EventKind::Error,
            serde_json::json!({ "message": format!("update failed: {error}") }),
        )),
    }
}

fn restart_into(root: &std::path::Path) {
    log_failure(
        crate::update::link_launcher(&crate::update::launcher_path(), root).err(),
        "launcher not linked",
    );
    let error = crate::update::reexec(root);
    tracing::error!(%error, "restart into the new build failed");
}
