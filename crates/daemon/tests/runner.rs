#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use api::driver::SessionDriver;
use api::routes::PromptImageDto;
use async_trait::async_trait;
use emberloop_core::events::{EventBus, EventKind};
use emberloop_core::steer::{SteerKind, SteerQueues};
use emberloop_daemon::runner::SessionRunner;
use futures::stream;
use harness::{
    EventStream, Harness, HarnessError, HarnessEvent, HarnessSession, HarnessWiring, PromptImage,
};
use prompts::PromptStore;
use proxy::{ProxyConfig, ProxyState};
use store::{DataRoot, EventLog, Project, ProjectsStore, SessionMeta, SessionsStore};
use tempfile::tempdir;

#[derive(Default)]
struct FakeHarness {
    wirings: Arc<Mutex<Vec<HarnessWiring>>>,
    /// What the fake reports for `resumed()`.
    resumes: bool,
    sent: Arc<Mutex<Vec<String>>>,
    woken: Arc<Mutex<Vec<String>>>,
    cancelled: Arc<Mutex<usize>>,
    events: Arc<Mutex<Vec<HarnessEvent>>>,
}

#[async_trait]
impl Harness for FakeHarness {
    async fn spawn(
        &self,
        _session_id: &str,
        wiring: HarnessWiring,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        self.wirings.lock().unwrap().push(wiring);
        let events = self.events.lock().unwrap().clone();
        Ok(Box::new(FakeSession {
            resumed: self.resumes,
            sent: Arc::clone(&self.sent),
            woken: Arc::clone(&self.woken),
            cancelled: Arc::clone(&self.cancelled),
            events,
        }))
    }
}

struct FakeSession {
    resumed: bool,
    sent: Arc<Mutex<Vec<String>>>,
    woken: Arc<Mutex<Vec<String>>>,
    cancelled: Arc<Mutex<usize>>,
    events: Vec<HarnessEvent>,
}

#[async_trait]
impl HarnessSession for FakeSession {
    async fn send(&self, prompt: &str, _images: Vec<PromptImage>) -> Result<(), HarnessError> {
        self.sent.lock().unwrap().push(prompt.to_string());
        Ok(())
    }

    fn events(&self) -> EventStream {
        Box::pin(stream::iter(self.events.clone()))
    }

    async fn cancel(&self) -> Result<(), HarnessError> {
        *self.cancelled.lock().unwrap() += 1;
        Ok(())
    }

    fn resume_token(&self) -> Option<String> {
        Some("tok-1".into())
    }

    fn resumed(&self) -> bool {
        self.resumed
    }

    async fn resume(&self, _token: &str) -> Result<(), HarnessError> {
        Ok(())
    }

    async fn wake(&self, text: &str) -> Result<(), HarnessError> {
        self.woken.lock().unwrap().push(text.to_string());
        Ok(())
    }

    async fn forget(&self, _resume_token: &str) -> Result<(), HarnessError> {
        Ok(())
    }
}

struct HarnessFixture {
    sessions: SessionsStore,
    runner: Arc<SessionRunner>,
    fake: Arc<FakeHarness>,
    events: Arc<EventBus>,
    steers: SteerQueues,
    data_root: DataRoot,
    _dir: tempfile::TempDir,
}

async fn runner_with(events_stream: Vec<HarnessEvent>) -> HarnessFixture {
    runner_resuming(events_stream, false).await
}

async fn runner_resuming(events_stream: Vec<HarnessEvent>, resumes: bool) -> HarnessFixture {
    let dir = tempdir().unwrap();
    let data_root = DataRoot::new(dir.path().join("data"));
    data_root.ensure_layout().await.unwrap();
    let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
    let projects = ProjectsStore::load(data_root.projects_file())
        .await
        .unwrap();
    projects
        .insert(Project {
            id: "proj_1".into(),
            name: "demo".into(),
            root: dir.path().to_path_buf(),
        })
        .await
        .unwrap();
    sessions
        .insert(SessionMeta {
            id: "ses_1".into(),
            project_id: "proj_1".into(),
            title: "Runner".into(),
            ponytail_level: "full".into(),
            created_at: 1,
            updated_at: 1,
            ..SessionMeta::default()
        })
        .await
        .unwrap();

    let events = Arc::new(EventBus::default());
    let steers = SteerQueues::default();
    let prompts = Arc::new(PromptStore::new(dir.path().join("prompts")));
    let models = Arc::new(api::models::ModelCatalogue::new(
        data_root.model_catalogue_file(),
        Vec::new(),
    ));
    let proxy = ProxyState::new(
        ProxyConfig::new(b"test-secret"),
        sessions.clone(),
        projects.clone(),
        data_root.clone(),
        prompts,
        models,
        Arc::clone(&events),
        steers.clone(),
        Arc::new(tokio::sync::RwLock::new(api::state::Settings::default())),
    );
    let fake = Arc::new(FakeHarness {
        resumes,
        events: Arc::new(Mutex::new(events_stream)),
        ..FakeHarness::default()
    });
    let logs = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
    let runner = Arc::new(SessionRunner::new(
        fake.clone(),
        sessions.clone(),
        projects,
        data_root.clone(),
        Arc::clone(&events),
        steers.clone(),
        proxy,
        "http://127.0.0.1:1".to_string(),
        logs,
        None,
        Arc::new(tokio::sync::RwLock::new(api::state::Settings::default())),
    ));
    HarnessFixture {
        sessions,
        runner,
        fake,
        events,
        steers,
        data_root,
        _dir: dir,
    }
}

async fn prompt_once(fixture: &HarnessFixture) {
    fixture
        .runner
        .prompt("ses_1", "hi", Vec::<PromptImageDto>::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn the_harness_token_is_stored_with_the_session() {
    let fixture = runner_with(Vec::new()).await;

    prompt_once(&fixture).await;

    let meta = fixture.sessions.get("ses_1").await.unwrap();
    assert_eq!(meta.resume_token.as_deref(), Some("tok-1"));
}

#[tokio::test]
async fn a_stored_token_and_a_stable_workdir_go_back_to_the_harness() {
    let fixture = runner_with(Vec::new()).await;
    let mut meta = fixture.sessions.get("ses_1").await.unwrap();
    meta.resume_token = Some("tok-old".into());
    fixture.sessions.save(&meta).await.unwrap();

    prompt_once(&fixture).await;

    let wiring = fixture.fake.wirings.lock().unwrap()[0].clone();
    assert_eq!(wiring.resume_token.as_deref(), Some("tok-old"));
    assert_eq!(
        wiring.workdir,
        Some(fixture.data_root.session_dir("ses_1").join("agent-work"))
    );
}

fn write_splices(data_root: &DataRoot) -> std::path::PathBuf {
    let path = data_root.session_splices_file("ses_1");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "{\"id\":\"old\",\"tool_result_id\":\"old\",\"marker\":\"m\",\"turns\":[]}\n",
    )
    .unwrap();
    path
}

#[tokio::test]
async fn a_fresh_conversation_drops_the_stored_proxy_history() {
    let fixture = runner_resuming(Vec::new(), false).await;
    let splices = write_splices(&fixture.data_root);

    prompt_once(&fixture).await;

    assert!(std::fs::read_to_string(&splices)
        .unwrap_or_default()
        .is_empty());
}

#[tokio::test]
async fn a_resumed_conversation_keeps_the_stored_proxy_history() {
    let fixture = runner_resuming(Vec::new(), true).await;
    let splices = write_splices(&fixture.data_root);

    prompt_once(&fixture).await;

    assert!(!std::fs::read_to_string(&splices).unwrap().is_empty());
}

async fn until(mut check: impl FnMut() -> bool) {
    for _ in 0..20_000 {
        if check() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("condition never became true");
}

#[tokio::test]
async fn a_prompt_reaches_the_harness_and_its_events_land_in_the_log() {
    let fixture = runner_with(vec![
        HarnessEvent::AssistantChunk("hello".into()),
        HarnessEvent::TurnFinished {
            stop_reason: Some("end".into()),
            usage: None,
        },
    ])
    .await;

    fixture
        .runner
        .prompt("ses_1", "do it", Vec::<PromptImageDto>::new())
        .await
        .unwrap();
    assert_eq!(fixture.fake.sent.lock().unwrap().as_slice(), ["do it"]);

    let dir = fixture.data_root.session_events_dir("ses_1");
    let mut chunks = 0;
    for _ in 0..20_000 {
        let log = EventLog::open(&dir).await.unwrap();
        let events = log.replay(None).await.unwrap();
        chunks = events
            .iter()
            .filter(|event| event.kind == EventKind::AssistantChunk)
            .count();
        if chunks == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(chunks, 1);
}

#[tokio::test]
async fn a_turn_end_wakes_pending_steers_and_notices_as_one_prompt() {
    let fixture = runner_with(vec![HarnessEvent::TurnFinished {
        stop_reason: Some("end".into()),
        usage: None,
    }])
    .await;
    let queue = fixture.steers.queue("ses_1");
    queue
        .enqueue(SteerKind::Steer, "steer: head to the parser")
        .await;
    queue
        .enqueue(SteerKind::Notice, "notice: background 7 exited")
        .await;
    fixture
        .runner
        .prompt("ses_1", "start", Vec::<PromptImageDto>::new())
        .await
        .unwrap();

    let mut delivered = fixture.events.subscribe();
    until(|| !fixture.fake.woken.lock().unwrap().is_empty()).await;
    let woken = fixture.fake.woken.lock().unwrap().clone();
    assert_eq!(woken.len(), 1);
    assert!(woken[0].contains("steer: head to the parser"));
    assert!(woken[0].contains("notice: background 7 exited"));

    let mut event = delivered.recv().await.unwrap();
    while event.kind != EventKind::SteerDelivered {
        event = delivered.recv().await.unwrap();
    }
    assert_eq!(event.data["route"], "sent at idle");
    assert!(queue
        .entries()
        .await
        .iter()
        .all(|entry| entry.route.is_some()));
}

#[tokio::test]
async fn a_turn_end_with_an_open_todo_reminds_the_model_once() {
    let fixture = runner_with(vec![HarnessEvent::TurnFinished {
        stop_reason: Some("end".into()),
        usage: None,
    }])
    .await;
    let dir = fixture.data_root.session_dir("ses_1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("todos.json"),
        r#"{"next":3,"items":[
            {"id":"t1","content":"read the spec","status":"completed"},
            {"id":"t2","content":"write the parser","status":"pending"}
        ]}"#,
    )
    .unwrap();
    fixture
        .runner
        .prompt("ses_1", "start", Vec::<PromptImageDto>::new())
        .await
        .unwrap();

    until(|| !fixture.fake.woken.lock().unwrap().is_empty()).await;
    let woken = fixture.fake.woken.lock().unwrap().clone();
    assert_eq!(woken.len(), 1);
    assert!(woken[0].contains("Reminder 1 of 3"));
    assert!(woken[0].contains("t2 pending"));
    assert!(!woken[0].contains("read the spec"));
}

#[tokio::test]
async fn a_finished_todo_call_republishes_the_plan_before_the_turn_ends() {
    let fixture = runner_with(vec![
        HarnessEvent::ToolStarted {
            id: "call-1".into(),
            name: "mcp__el__todo".into(),
            input: serde_json::json!({ "action": "set" }),
        },
        HarnessEvent::ToolFinished {
            id: "call-1".into(),
            output: serde_json::json!(
                "1 pending · 0 in progress · 0 completed · 0 cancelled\nt1 [ ] one\n"
            ),
            status: None,
        },
    ])
    .await;
    let dir = fixture.data_root.session_dir("ses_1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("todos.json"),
        r#"{"next":2,"items":[{"id":"t1","content":"one","status":"pending"}]}"#,
    )
    .unwrap();
    let mut events = fixture.events.subscribe();
    fixture
        .runner
        .prompt("ses_1", "start", Vec::<PromptImageDto>::new())
        .await
        .unwrap();

    // No TurnFinished here: the only todos event can be the one after the call.
    loop {
        let event = events.recv().await.unwrap();
        if event.kind == EventKind::Todos {
            assert_eq!(event.data["items"][0]["id"], "t1");
            break;
        }
    }
}

#[tokio::test]
async fn cancel_and_connect_reach_the_harness() {
    let fixture = runner_with(Vec::new()).await;
    fixture.runner.connect("ses_1").await.unwrap();
    fixture.runner.cancel("ses_1").await.unwrap();
    assert_eq!(*fixture.fake.cancelled.lock().unwrap(), 1);
    assert!(fixture.fake.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_notice_file_becomes_a_steer_at_the_next_turn_end() {
    let fixture = runner_with(vec![HarnessEvent::TurnFinished {
        stop_reason: Some("end".into()),
        usage: None,
    }])
    .await;
    std::fs::write(
        fixture.data_root.session_dir("ses_1").join("notices.jsonl"),
        "{\"text\":\"background bg_1 exited (code 0)\"}\n",
    )
    .unwrap();

    fixture
        .runner
        .prompt("ses_1", "start", Vec::<PromptImageDto>::new())
        .await
        .unwrap();
    until(|| !fixture.fake.woken.lock().unwrap().is_empty()).await;
    let woken = fixture.fake.woken.lock().unwrap().clone();
    assert!(woken[0].contains("background bg_1 exited (code 0)"));
    assert!(!fixture
        .data_root
        .session_dir("ses_1")
        .join("notices.jsonl")
        .metadata()
        .map(|meta| meta.len() > 0)
        .unwrap_or(false));
}
