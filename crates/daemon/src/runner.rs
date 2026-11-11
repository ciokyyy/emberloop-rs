use std::collections::HashMap;
use std::sync::Arc;

use crate::idle::{self, Activity, Clock, SystemClock, DEFAULT_IDLE_TIMEOUT};
use crate::todo_reminder::{self, Cycle};
use crate::tooldisplay::ToolTracker;
use api::driver::SessionDriver;
use api::routes::PromptImageDto;
use api::state::Settings;
use emberloop_core::events::{EventBus, EventKind, SessionEvent};
use emberloop_core::steer::{
    deliver_idle, SteerImage, SteerKind, SteerQueue, SteerQueues, SteerWaker,
};
use futures::StreamExt;
use harness::{Harness, HarnessEvent, HarnessSession, HarnessWiring, PromptImage};
use prompts::PromptStore;
use proxy::ProxyState;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use store::{DataRoot, EventLog, ProjectsStore, SessionMeta, SessionsStore};
use tokio::sync::{Mutex, RwLock};

/// How often the idle watcher checks the Sessions.
const IDLE_CHECK_EVERY: Duration = Duration::from_secs(30);

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
    settings: Arc<RwLock<Settings>>,
    reminders: Arc<Mutex<HashMap<String, Cycle>>>,
    titles: Option<crate::titles::Titles>,
    models: Option<Arc<api::models::ModelCatalogue>>,
    config_root: Option<std::path::PathBuf>,
    global_skills: Option<std::path::PathBuf>,
    prompts: Option<Arc<PromptStore>>,
    clock: Arc<dyn Clock>,
    idle_timeout: Duration,
}

fn log_failure(error: Option<impl std::fmt::Display>, what: &str) {
    if let Some(error) = error {
        tracing::warn!(%error, "{what}");
    }
}

struct Handle {
    session: Arc<dyn HarnessSession>,
    queue: Arc<SteerQueue>,
    /// Wakes the pump to drop the harness when the Session moves to another one.
    stop: tokio::sync::Notify,
    clock: Arc<dyn Clock>,
    /// Set on every use and when a turn ends; the idle reaper counts from here.
    last_active: StdMutex<Instant>,
}

impl Handle {
    fn new(
        session: Arc<dyn HarnessSession>,
        queue: Arc<SteerQueue>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let last_active = StdMutex::new(clock.now());
        Self {
            session,
            queue,
            stop: tokio::sync::Notify::new(),
            clock,
            last_active,
        }
    }

    fn touch(&self) {
        let now = self.clock.now();
        if let Ok(mut last) = self.last_active.lock() {
            *last = now;
        }
    }

    fn idle_since(&self) -> Instant {
        self.last_active
            .lock()
            .map_or_else(|_| self.clock.now(), |last| *last)
    }
}

/// Stop a Session's harness: the pump ends, then the harness is closed and its process waited for.
async fn retire(handle: &Handle) {
    handle.stop.notify_one();
    if let Err(error) = handle.session.close().await {
        tracing::warn!(%error, "harness did not close cleanly");
    }
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
        settings: Arc<RwLock<Settings>>,
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
            settings,
            reminders: Arc::new(Mutex::new(HashMap::new())),
            titles: None,
            models: None,
            config_root: None,
            global_skills: None,
            prompts: None,
            clock: Arc::new(SystemClock),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }

    /// The clock the idle reaper reads; tests inject one that moves by hand.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// How long a Session may sit idle before its harness is closed.
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Closes every Session whose harness has been idle past the threshold, and returns their IDs.
    /// A Session with a running turn, a pending steer or a live background job is never closed.
    /// It resumes through the normal spawn path on its next message.
    pub async fn reap_idle(&self) -> Vec<String> {
        let now = self.clock.now();
        let mut reaped = Vec::new();
        {
            // The decision and the removal share one lock, so a prompt that finds its
            // handle after this point spawns a fresh harness.
            let mut handles = self.handles.lock().await;
            let idle = self.idle_ids(&handles, now).await;
            for id in idle {
                if let Some(handle) = handles.remove(&id) {
                    reaped.push((id, handle));
                }
            }
        }
        let mut ids = Vec::with_capacity(reaped.len());
        for (id, handle) in reaped {
            retire(&handle).await;
            tracing::info!(session = %id, "closed the idle harness");
            ids.push(id);
        }
        ids
    }

    async fn idle_ids(&self, handles: &HashMap<String, Arc<Handle>>, now: Instant) -> Vec<String> {
        let mut idle = Vec::new();
        for (id, handle) in handles {
            let activity = Activity {
                turn_running: handle.queue.is_turn_active(),
                steers_pending: handle.queue.pending_len().await > 0,
                background_running: tools::background::any_running(
                    &self.data_root.session_spill_dir(id),
                ),
                last_active: handle.idle_since(),
            };
            if idle::reapable(activity, now, self.idle_timeout) {
                idle.push(id.clone());
            }
        }
        idle
    }

    /// Starts the idle watcher. It is the only caller of `reap_idle` outside tests.
    #[allow(clippy::disallowed_methods)]
    pub fn watch_idle(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let runner = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                runner.reap_idle().await;
                tokio::time::sleep(IDLE_CHECK_EVERY).await;
            }
        })
    }

    /// Forgets a handle the runner no longer trusts and closes its harness, so the next
    /// message spawns a new one. The handle is removed only while it is still the current one.
    async fn evict(&self, session_id: &str, handle: &Arc<Handle>) {
        let removed = {
            let mut handles = self.handles.lock().await;
            match handles.get(session_id) {
                Some(current) if Arc::ptr_eq(current, handle) => handles.remove(session_id),
                _ => None,
            }
        };
        if let Some(removed) = removed {
            retire(&removed).await;
        }
    }

    pub fn with_global_skills(mut self, root: std::path::PathBuf) -> Self {
        self.global_skills = Some(root);
        self
    }

    pub fn with_prompts(mut self, prompts: Arc<PromptStore>) -> Self {
        self.prompts = Some(prompts);
        self
    }

    pub(crate) fn with_config_root(mut self, root: std::path::PathBuf) -> Self {
        self.config_root = Some(root);
        self
    }

    pub(crate) fn with_models(mut self, models: Arc<api::models::ModelCatalogue>) -> Self {
        self.models = Some(models);
        self
    }

    pub(crate) fn with_titles(mut self, titles: crate::titles::Titles) -> Self {
        self.titles = Some(titles);
        self
    }

    /// Starts the idle watcher for updates queued from a terminal.
    pub fn watch_updates(&self) -> tokio::task::JoinHandle<()> {
        watch_updates(
            self.data_root.clone(),
            Arc::clone(&self.handles),
            Arc::clone(&self.events),
        )
    }

    fn wiring(
        &self,
        session_id: &str,
        root: &std::path::Path,
        meta: &SessionMeta,
        image_input: bool,
        ctx_spill_enabled: bool,
    ) -> HarnessWiring {
        let spill = self.data_root.session_spill_dir(session_id);
        let mcp_command = crate::daemon_exe().map(|exe| {
            // The Session's HOME is a harness pi-home, so name the daemon's real config root.
            let mut argv = vec![exe.display().to_string()];
            if let Some(config_root) = &self.config_root {
                argv.push("--config-root".to_string());
                argv.push(config_root.display().to_string());
            }
            argv.extend([
                "mcp".to_string(),
                "--root".to_string(),
                root.display().to_string(),
                "--spill".to_string(),
                spill.display().to_string(),
            ]);
            if let Some(global) = &self.global_skills {
                argv.push("--global-skills".to_string());
                argv.push(global.display().to_string());
            }
            argv
        });
        HarnessWiring {
            proxy_url: format!(
                "{}/proxy/{}",
                self.proxy_base.trim_end_matches('/'),
                self.proxy.token_for(session_id)
            ),
            mcp_url: None,
            mcp_command,
            env: vec![
                (
                    "EMBERLOOP_SESSION_URL".into(),
                    format!(
                        "{}/api/sessions/{session_id}",
                        self.proxy_base.trim_end_matches('/')
                    ),
                ),
                (
                    "EMBERLOOP_REVIEWER".into(),
                    if meta.reviewer.is_some() { "1" } else { "0" }.into(),
                ),
                (
                    "EMBERLOOP_IMAGE_INPUT".into(),
                    if image_input { "1" } else { "0" }.into(),
                ),
                (
                    "EMBERLOOP_CTX_SPILL".into(),
                    if ctx_spill_enabled { "1" } else { "0" }.into(),
                ),
            ],
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
        self.save_token(meta, session).await;
    }

    /// The CLI reports its conversation id only once the first message is out,
    /// so the token is settled again after a turn starts.
    async fn save_token(&self, meta: &SessionMeta, session: &dyn HarnessSession) {
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

    /// Writes the Session's skill catalog once, before its harness starts. A skill made later
    /// reaches the next Session, never this one.
    fn freeze_catalog(&self, session_id: &str, project_root: &std::path::Path) {
        let dir = self.data_root.session_dir(session_id);
        let path = dir.join("skill-catalog.md");
        if path.exists() {
            return;
        }
        let intro = self
            .prompts
            .as_ref()
            .and_then(|store| store.text(prompts::SKILL_CATALOG))
            .unwrap_or_default();
        let global = self.global_skills.as_deref();
        let text = tools::skills::catalog(&intro, global, project_root);
        let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text));
        if let Err(error) = written {
            tracing::warn!(%error, %session_id, "could not write the skill catalog");
        }
    }

    async fn ensure(&self, session_id: &str) -> Result<Arc<Handle>, String> {
        let existing = self.handles.lock().await.get(session_id).cloned();
        if let Some(handle) = existing {
            handle.touch();
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
        let image_input = match &self.models {
            Some(models) => models.image_input(&meta.model).await.unwrap_or(true),
            None => true,
        };
        let ctx_spill_enabled = self.settings.read().await.ctx_spill_enabled;
        let wiring = self.wiring(
            session_id,
            &project.root,
            &meta,
            image_input,
            ctx_spill_enabled,
        );
        self.freeze_catalog(session_id, &project.root);
        let session = Arc::<dyn HarnessSession>::from(
            self.harness
                .spawn(session_id, wiring)
                .await
                .map_err(|error| error.to_string())?,
        );
        self.settle_conversation(session_id, &meta, session.as_ref())
            .await;
        let handle = Arc::new(Handle::new(
            session,
            self.steers.queue(session_id),
            Arc::clone(&self.clock),
        ));

        let mut handles = self.handles.lock().await;
        if let Some(existing) = handles.get(session_id).cloned() {
            // Another prompt spawned first; this harness is never used, so close it.
            drop(handles);
            retire(&handle).await;
            return Ok(existing);
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
            Arc::clone(&self.settings),
            Arc::clone(&self.reminders),
            self.proxy.prompts(),
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

    /// A prompt that never reached the harness still has to end its turn on screen: without the
    /// error and the idle status the Session stays "working" forever.
    async fn fail_turn(&self, session_id: &str, message: String) -> String {
        let events_dir = self.data_root.session_events_dir(session_id);
        let events = [
            (EventKind::Error, serde_json::json!({ "message": message })),
            (EventKind::Status, serde_json::json!({ "state": "idle" })),
        ];
        for (kind, data) in events {
            let event = SessionEvent::new(session_id, 0, kind, data);
            if let Ok(stored) = append(&self.logs, &events_dir, &event).await {
                self.events.publish(stored);
            }
        }
        message
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
        let is_agy =
            self.sessions.get(session_id).await.is_some_and(|meta| {
                crate::harness_kind(Some(&meta.model)) == crate::HarnessKind::Agy
            });
        if is_agy {
            if let Some(home) = &self.account_home {
                if !home.join("antigravity-acp/acp_token.json").exists() {
                    let message = "no Antigravity account is signed in; open Providers to sign in";
                    return Err(self.fail_turn(session_id, message.into()).await);
                }
            }
        }
        self.drain_notices(session_id).await;
        self.reminders.lock().await.remove(session_id);
        let handle = match self.ensure(session_id).await {
            Ok(handle) => handle,
            Err(error) => return Err(self.fail_turn(session_id, error.to_string()).await),
        };
        let images: Vec<PromptImage> = images
            .into_iter()
            .map(|image| PromptImage {
                mime_type: image.mime_type,
                data_base64: image.data_base64,
            })
            .collect();
        if let Some(titles) = &self.titles {
            titles.start(session_id, text).await;
        }
        handle.queue.set_turn_active(true);
        if let Err(error) = handle.session.send(text, images).await {
            handle.queue.set_turn_active(false);
            self.evict(session_id, &handle).await;
            return Err(self.fail_turn(session_id, error.to_string()).await);
        }
        // The CLI reports its conversation id only once the first message is out.
        if let Some(meta) = self.sessions.get(session_id).await {
            self.save_token(&meta, handle.session.as_ref()).await;
        }
        Ok(())
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

    async fn wake(
        &self,
        session_id: &str,
        text: &str,
        images: Vec<PromptImageDto>,
    ) -> Result<(), String> {
        let handle = self.ensure(session_id).await?;
        handle.queue.set_turn_active(true);
        let images = images
            .into_iter()
            .map(|image| PromptImage {
                mime_type: image.mime_type,
                data_base64: image.data_base64,
            })
            .collect();
        let woken = handle.session.wake(text, images).await;
        if woken.is_err() {
            handle.queue.set_turn_active(false);
            self.evict(session_id, &handle).await;
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

    async fn configure_model(
        &self,
        session_id: &str,
        model: &str,
        effort: Option<&str>,
    ) -> Result<(), String> {
        let meta = self
            .sessions
            .get(session_id)
            .await
            .ok_or("Session not found")?;
        if self.steers.queue(session_id).is_turn_active() {
            return Err("Wait for the current turn to finish".into());
        }
        if crate::harness_kind(Some(&meta.model)) != crate::harness_kind(Some(model)) {
            // The next prompt spawns the new harness; the proxy carries the history over.
            let removed = self.handles.lock().await.remove(session_id);
            if let Some(handle) = removed {
                retire(&handle).await;
            }
            return Ok(());
        }
        if let Some(handle) = self.handle(session_id).await {
            if let Err(error) = handle.session.configure_model(model, effort).await {
                let _ = handle
                    .session
                    .configure_model(&meta.model, meta.thought_level.as_deref())
                    .await;
                return Err(error.to_string());
            }
        }
        Ok(())
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

/// The longest an event waits for the persister's backlog before it is numbered anyway.
const PERSIST_BACKLOG_LIMIT: std::time::Duration = std::time::Duration::from_millis(250);

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
    settings: Arc<RwLock<Settings>>,
    reminders: Arc<Mutex<HashMap<String, Cycle>>>,
    prompts: Arc<PromptStore>,
) {
    let mut stream = handle.session.events();
    let mut tracker = ToolTracker::default();
    loop {
        let next = tokio::select! {
            next = stream.next() => next,
            () = handle.stop.notified() => None,
        };
        let Some(event) = next else { break };
        let turn_over = note_turn_state(&handle, &event);
        if let Some(mapped) = map_event(&session_id, event, &mut tracker, &project_root) {
            publish_mapped(&events, &logs, &events_dir, &session_id, &data_root, mapped).await;
        }
        if turn_over {
            handle.touch();
            drain_notices(&data_root, &steers, &session_id).await;
            apply_pending_update(&data_root, &handles, &events, Some(&session_id)).await;
            turn_end_todos(
                &handle,
                &session_id,
                &data_root,
                &events,
                &logs,
                &events_dir,
                &settings,
                &reminders,
                &prompts,
            )
            .await;
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
    forget_handle(&handles, &session_id, &handle).await;
}

/// Clears the running-turn flag when the event ends a turn or fails it; true when the turn finished.
fn note_turn_state(handle: &Handle, event: &HarnessEvent) -> bool {
    let finished = matches!(event, HarnessEvent::TurnFinished { .. });
    if finished || matches!(event, HarnessEvent::Error(_)) {
        handle.queue.set_turn_active(false);
    }
    finished
}

/// The harness ended or was closed: forget its handle so the next message spawns one.
async fn forget_handle(
    handles: &Mutex<HashMap<String, Arc<Handle>>>,
    session_id: &str,
    handle: &Arc<Handle>,
) {
    let mut handles = handles.lock().await;
    if handles
        .get(session_id)
        .is_some_and(|current| Arc::ptr_eq(current, handle))
    {
        handles.remove(session_id);
    }
}

/// Append one mapped event and publish it, then republish the plan when a `todo`
/// call finished, so the plan card follows the model inside a turn.
async fn publish_mapped(
    events: &EventBus,
    logs: &Arc<RwLock<HashMap<String, EventLog>>>,
    events_dir: &std::path::Path,
    session_id: &str,
    data_root: &DataRoot,
    mapped: SessionEvent,
) {
    // The harness sends the full MCP name (`mcp__el__todo`, `el_todo`); the plan
    // follows the short one.
    let todo_changed = mapped.kind == EventKind::ToolFinished
        && mapped
            .data
            .get("name")
            .and_then(|name| name.as_str())
            .is_some_and(|name| crate::tooldisplay::short_tool_name(name) == "todo");
    // The proxy's thinking is numbered by the persister, a hop behind; this event
    // happened after them, so it takes its number after theirs.
    events.settled(PERSIST_BACKLOG_LIMIT).await;
    let stored = match append(logs, events_dir, &mapped).await {
        Ok(stored) => stored,
        Err(error) => {
            let failure = SessionEvent::new(
                session_id,
                0,
                EventKind::Error,
                serde_json::json!({ "message": format!("event log error: {error}") }),
            );
            append(logs, events_dir, &failure).await.unwrap_or(failure)
        }
    };
    events.publish(stored);
    if todo_changed {
        publish_todos(events, logs, events_dir, session_id, data_root).await;
    }
}

/// Read the stored plan and publish it as one `todos` event. The daemon never
/// writes the list; the tool process owns `todos.json`.
async fn publish_todos(
    events: &EventBus,
    logs: &Arc<RwLock<HashMap<String, EventLog>>>,
    events_dir: &std::path::Path,
    session_id: &str,
    data_root: &DataRoot,
) {
    let path = data_root.session_dir(session_id).join("todos.json");
    let items = tools::todo::load_from(&path);
    let event = SessionEvent::new(
        session_id,
        0,
        EventKind::Todos,
        serde_json::json!({ "items": items }),
    );
    if let Ok(stored) = append(logs, events_dir, &event).await {
        events.publish(stored);
    }
}

/// Turn-end reminder: publish the plan, then enqueue one `Notice` before the idle
/// wake, so a queued steer and the reminder go out as one prompt.
///
/// A question pending for the user is not visible to the daemon: the harness
/// exposes no such signal, so a queued steer or a running background job is the
/// whole settled check. See `crates/daemon/AGENTS.md`.
#[allow(clippy::too_many_arguments)]
async fn turn_end_todos(
    handle: &Handle,
    session_id: &str,
    data_root: &DataRoot,
    events: &EventBus,
    logs: &Arc<RwLock<HashMap<String, EventLog>>>,
    events_dir: &std::path::Path,
    settings: &Arc<RwLock<Settings>>,
    reminders: &Arc<Mutex<HashMap<String, Cycle>>>,
    prompts: &PromptStore,
) {
    publish_todos(events, logs, events_dir, session_id, data_root).await;
    let settings = settings.read().await.clone();
    let path = data_root.session_dir(session_id).join("todos.json");
    let items = tools::todo::load_from(&path);
    let spill = data_root.session_spill_dir(session_id);
    if handle.queue.pending_len().await > 0 || tools::background::any_running(&spill) {
        return;
    }
    let mut cycles = reminders.lock().await;
    let cycle = cycles.entry(session_id.to_string()).or_default();
    if todo_reminder::should_send(&settings, cycle, &items) {
        send_reminder(handle, cycle, &items, &settings, prompts).await;
        return;
    }
    let open = todo_reminder::open_set(&items).len();
    if todo_reminder::should_cap(&settings, cycle, open) {
        cycle.capped = true;
        drop(cycles);
        let text = todo_reminder::cap_note(open, settings.todo_reminders_max);
        publish_notice(events, logs, events_dir, session_id, text, "cap").await;
    }
}

/// Enqueue one reminder and count it; a template that fails to render sends nothing and
/// leaves the cycle unchanged, so the next settled turn end tries again.
async fn send_reminder(
    handle: &Handle,
    cycle: &mut Cycle,
    items: &[tools::todo::Todo],
    settings: &Settings,
    prompts: &PromptStore,
) {
    let sent = cycle.sent + 1;
    match todo_reminder::text(prompts, items, sent, settings.todo_reminders_max) {
        Ok(text) => {
            cycle.sent = sent;
            cycle.last_open = todo_reminder::open_set(items);
            handle.queue.enqueue(SteerKind::Notice, text).await;
        }
        Err(error) => tracing::warn!(%error, "todo reminder not sent"),
    }
}

/// Store one feed notice and publish it.
async fn publish_notice(
    events: &EventBus,
    logs: &Arc<RwLock<HashMap<String, EventLog>>>,
    events_dir: &std::path::Path,
    session_id: &str,
    text: String,
    route: &str,
) {
    let event = SessionEvent::new(
        session_id,
        0,
        EventKind::Notice,
        serde_json::json!({ "text": text, "route": route }),
    );
    if let Ok(stored) = append(logs, events_dir, &event).await {
        events.publish(stored);
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
    async fn wake(&self, text: &str, images: &[SteerImage]) -> Result<(), String> {
        self.handle.queue.set_turn_active(true);
        let images = images
            .iter()
            .map(|image| PromptImage {
                mime_type: image.mime_type.clone(),
                data_base64: image.data_base64.clone(),
            })
            .collect();
        let res = self
            .handle
            .session
            .wake(text, images)
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

/// Apply a queued update once its requester is idle (all Sessions for an external request).
/// Active Sessions are saved in the update marker for continuation after restart.
async fn apply_pending_update(
    data_root: &DataRoot,
    handles: &Mutex<HashMap<String, Arc<Handle>>>,
    events: &EventBus,
    session_id: Option<&str>,
) {
    let active: Vec<String> = handles
        .lock()
        .await
        .iter()
        .filter(|(_, handle)| handle.queue.is_turn_active())
        .map(|(id, _)| id.clone())
        .collect();
    let Some(request) = crate::update::read_request(data_root.root()) else {
        return;
    };
    // Only the requester has to be idle; other running Sessions are cut off by the restart and
    // resumed by the new daemon. A request with no requester still waits for everyone.
    let busy = match &request.session {
        Some(requester) => active.contains(requester),
        None => !active.is_empty(),
    };
    if crate::update::due(data_root.root(), busy).is_none() {
        return;
    }
    let also: Vec<String> = active
        .into_iter()
        .filter(|id| Some(id) != request.session.as_ref())
        .collect();
    let root = crate::update::install_root();
    match crate::update::apply_request(&root, data_root.root(), also).await {
        Ok(Some(_)) => restart_into(&root),
        Ok(None) => {}
        Err(error) => {
            let message = format!("update failed: {error}");
            match session_id {
                Some(id) => events.publish(SessionEvent::new(
                    id,
                    0,
                    EventKind::Error,
                    serde_json::json!({ "message": message }),
                )),
                None => tracing::error!(%message),
            }
        }
    }
}

/// An `emberloop update` run from a terminal only queues; this picks it up while the daemon is
/// idle. The first pass also covers a request queued while the daemon was down.
#[allow(clippy::disallowed_methods)]
fn watch_updates(
    data_root: DataRoot,
    handles: Arc<Mutex<HashMap<String, Arc<Handle>>>>,
    events: Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            apply_pending_update(&data_root, &handles, &events, None).await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    })
}

fn restart_into(root: &std::path::Path) {
    log_failure(
        crate::update::link_launcher(&crate::update::launcher_path(), root).err(),
        "launcher not linked",
    );
    let error = crate::update::reexec(root);
    tracing::error!(%error, "restart into the new build failed");
}
