use super::*;
use axum::body::{Body, Bytes};
use axum::extract::State as AxumState;
use axum::http::{header, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router as AxumRouter};
use emberloop_core::events::{EventBus, EventKind};
use futures::Stream;
use prompts::PromptStore;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use store::{DataRoot, Project, ProjectsStore, SessionMeta, SessionsStore};
use tempfile::tempdir;
use tower::ServiceExt;

#[derive(Clone)]
struct FakeUpstream {
    calls: Arc<AtomicUsize>,
    bodies: Arc<Mutex<Vec<Value>>>,
    sse: Arc<Mutex<VecDeque<String>>>,
    hang: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    /// The next call answers 500 instead of its usual response.
    fail_next: Arc<AtomicBool>,
    /// Calls never answer while set, so a client that gives up cancels the request.
    stall: Arc<AtomicBool>,
    /// Signalled when a stalled call arrives, so a test knows the request is in flight.
    arrived: Arc<tokio::sync::Notify>,
    /// Answers taken in order before the default answer.
    script: Arc<Mutex<VecDeque<Value>>>,
    /// The default text answer reports the request's size as its input usage, as a provider does.
    echo_usage: Arc<AtomicBool>,
}

impl FakeUpstream {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            bodies: Arc::new(Mutex::new(Vec::new())),
            sse: Arc::new(Mutex::new(VecDeque::new())),
            hang: Arc::new(Mutex::new(None)),
            fail_next: Arc::new(AtomicBool::new(false)),
            stall: Arc::new(AtomicBool::new(false)),
            arrived: Arc::new(tokio::sync::Notify::new()),
            script: Arc::new(Mutex::new(VecDeque::new())),
            echo_usage: Arc::new(AtomicBool::new(false)),
        }
    }

    fn sse(responses: Vec<String>) -> Self {
        let mut fake = Self::new();
        fake.sse = Arc::new(Mutex::new(responses.into()));
        fake
    }

    fn hanging(dropped: Arc<AtomicBool>) -> Self {
        let mut fake = Self::new();
        fake.hang = Arc::new(Mutex::new(Some(dropped)));
        fake
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn body(&self, index: usize) -> Value {
        self.bodies.lock().unwrap()[index].clone()
    }
}

async fn fake_upstream(
    AxumState(fake): AxumState<FakeUpstream>,
    Json(body): Json<Value>,
) -> Response {
    if let Some(dropped) = fake.hang.lock().unwrap().clone() {
        fake.calls.fetch_add(1, Ordering::SeqCst);
        fake.bodies.lock().unwrap().push(body.clone());
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(HangStream {
                first: true,
                dropped,
            }))
            .unwrap();
    }
    if fake.stall.load(Ordering::SeqCst) {
        fake.arrived.notify_one();
        std::future::pending::<()>().await;
    }
    fake.calls.fetch_add(1, Ordering::SeqCst);
    let body_size = body.to_string().len();
    fake.bodies.lock().unwrap().push(body);
    if fake.fail_next.swap(false, Ordering::SeqCst) {
        return (StatusCode::INTERNAL_SERVER_ERROR, "upstream failed").into_response();
    }
    if let Some(response) = fake.sse.lock().unwrap().pop_front() {
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(response))
            .unwrap();
    }
    if let Some(response) = fake.script.lock().unwrap().pop_front() {
        return Json(response).into_response();
    }
    let call = fake.calls.load(Ordering::SeqCst) - 1;
    let prompt_tokens = if fake.echo_usage.load(Ordering::SeqCst) {
        u64::try_from(body_size).unwrap_or(0) / 4
    } else {
        11
    };
    let json = if call == 0 {
        json!({
            "response": {
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{
                            "functionCall": {
                                "id": "call_1",
                                "name": "compress",
                                "args": { "start": 0, "end": 2, "summary": "folded", "topic": "early" }
                            },
                            "thoughtSignature": "SIG-1"
                        }]
                    }
                }]
            }
        })
    } else {
        json!({
            "response": {
                "candidates": [{
                    "content": { "role": "model", "parts": [{ "text": "done" }] }
                }]
            },
            "usageMetadata": {
                "promptTokenCount": prompt_tokens,
                "candidatesTokenCount": 7,
                "totalTokenCount": prompt_tokens + 7
            }
        })
    };
    Json(json).into_response()
}

fn sse_event(parts: Value) -> String {
    format!(
        "data: {}\n\n",
        json!({"response": {"candidates": [{"content": {"role": "model", "parts": parts}}]}})
    )
}

struct FakeModels(Vec<api::models::ModelEntry>);

#[async_trait::async_trait]
impl api::models::Source for FakeModels {
    fn name(&self) -> &'static str {
        "agy"
    }

    async fn fetch(&self) -> Result<Vec<api::models::ModelEntry>, api::models::CatalogueError> {
        Ok(self.0.clone())
    }
}

struct HangStream {
    first: bool,
    dropped: Arc<AtomicBool>,
}

impl Stream for HangStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.first {
            self.first = false;
            return Poll::Ready(Some(Ok(Bytes::from(sse_event(json!([{
                "functionCall": {
                    "id": "hang_1",
                    "name": "compress",
                    "args": {"start": 0, "end": 1, "summary": "hang"}
                }
            }]))))));
        }
        Poll::Pending
    }
}

impl Drop for HangStream {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

struct Fixture {
    state: ProxyState,
    steers: emberloop_core::steer::SteerQueues,
    sessions: SessionsStore,
    projects: ProjectsStore,
    models: Arc<api::models::ModelCatalogue>,
    prompts: Arc<PromptStore>,
    data_root: DataRoot,
    config: ProxyConfig,
    events: Arc<EventBus>,
    settings: Arc<tokio::sync::RwLock<api::state::Settings>>,
    token: String,
    fake: FakeUpstream,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    fixture_with(FakeUpstream::new()).await
}

async fn fixture_sse(responses: Vec<String>) -> Fixture {
    fixture_with(FakeUpstream::sse(responses)).await
}

async fn fixture_with(fake: FakeUpstream) -> Fixture {
    fixture_config(fake, |config| config).await
}

async fn fixture_config(
    fake: FakeUpstream,
    configure: impl FnOnce(ProxyConfig) -> ProxyConfig,
) -> Fixture {
    let dir = tempdir().unwrap();
    let data_root = DataRoot::new(dir.path());
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
            title: "Proxy".into(),
            ponytail_level: "full".into(),
            created_at: 1,
            updated_at: 1,
            prompt_hashes: HashMap::new(),
            ..SessionMeta::default()
        })
        .await
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let upstream = AxumRouter::new()
        .route("/v1internal:generateContent", post(fake_upstream))
        .route("/v1internal:streamGenerateContent", post(fake_upstream))
        .layer(axum::extract::DefaultBodyLimit::max(128 * 1024 * 1024))
        .with_state(fake.clone());
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let events = Arc::new(EventBus::new(16));
    let prompts = Arc::new(PromptStore::new(dir.path().join("prompts")));
    let models = Arc::new(api::models::ModelCatalogue::new(
        data_root.model_catalogue_file(),
        vec![Arc::new(FakeModels(vec![api::models::ModelEntry {
            id: "gemini-3.8-flash".into(),
            provider: "agy".into(),
            label: "Gemini 3.8 Flash".into(),
            kind: "chat".into(),
            efforts: Vec::new(),
            image_input: false,
            context_window: 1_000_000,
            source: "agy".into(),
            stale: false,
            updated_at: 0,
        }]))],
    ));
    models.refresh().await.unwrap();
    let steers = emberloop_core::steer::SteerQueues::default();
    let config =
        configure(ProxyConfig::new(b"test-secret").with_upstream(format!("http://{addr}")));
    let data_root_for_projects = data_root.clone();
    let data_root_for_fixture = data_root.clone();
    let models_for_fixture = models.clone();
    let prompts_for_fixture = prompts.clone();
    let config_for_fixture = config.clone();
    let settings = Arc::new(tokio::sync::RwLock::new(api::state::Settings {
        preserve_recent_messages: 0,
        ..api::state::Settings::default()
    }));
    let state = ProxyState::new(
        config,
        sessions.clone(),
        projects,
        data_root,
        prompts,
        models,
        events.clone(),
        steers.clone(),
        settings.clone(),
    );
    let token = state.token_for("ses_1");

    Fixture {
        state,
        steers,
        sessions,
        projects: ProjectsStore::load(data_root_for_projects.projects_file())
            .await
            .unwrap(),
        models: models_for_fixture,
        prompts: prompts_for_fixture,
        data_root: data_root_for_fixture,
        config: config_for_fixture,
        events,
        settings,
        token,
        fake,
        _dir: dir,
    }
}

fn conversation() -> Value {
    json!([
        { "role": "user", "parts": [{ "text": "one" }] },
        { "role": "model", "parts": [{ "text": "two" }] },
        { "role": "user", "parts": [{ "text": "three" }] }
    ])
}

fn body(contents: Value) -> Value {
    json!({ "contents": contents })
}

async fn send(state: &ProxyState, token: &str, path: &str, body: Value) -> (StatusCode, Bytes) {
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/proxy/{token}/{path}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, bytes)
}

#[tokio::test]
async fn rejects_a_tampered_session_token() {
    let fixture = fixture().await;
    let (status, _) = send(
        &fixture.state,
        "00ff",
        "v1internal:generateContent",
        body(conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(fixture.fake.calls(), 0);
}

#[tokio::test]
async fn injects_tools_prompts_and_meters_usage() {
    let fixture = fixture().await;
    let mut events = fixture.events.subscribe();

    let (status, response) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&response).contains("done"));

    let upstream = fixture.fake.body(0);
    let declarations = upstream["tools"][0]["functionDeclarations"]
        .as_array()
        .unwrap();
    let names: Vec<&str> = declarations
        .iter()
        .filter_map(|declaration| declaration["name"].as_str())
        .collect();
    assert!(names.contains(&"compress"));
    assert!(names.contains(&"decompress"));
    assert!(names.contains(&"search_context"));

    let system = upstream["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap();
    assert!(system.contains("Ponytail"));
    assert!(system.contains("CAVEMAN MODE ACTIVE — level: ultra"));
    assert!(system.contains("demo"));

    let comp_event = events.try_recv().expect("compression event");
    assert_eq!(comp_event.kind, EventKind::Compression);
    let event = events.try_recv().expect("usage event");
    assert_eq!(event.kind, EventKind::Usage);
    assert_eq!(event.data["inputTokens"], 11);
    assert_eq!(event.data["outputTokens"], 7);
    // The meter carries the window the catalogue knows for the Session's model.
    assert_eq!(event.data["contextWindow"], 1_000_000);
    assert!(event.data["breakdown"]["system"].as_u64().unwrap() > 0);
    assert!(event.data["breakdown"]["toolDefinitions"].as_u64().unwrap() > 0);
    assert!(event.data["breakdown"]["compression"].as_u64().unwrap() > 0);

    let meta = fixture.sessions.get("ses_1").await.unwrap();
    assert!(!meta.prompt_hashes.is_empty());
    assert!(meta.prompt_hashes.contains_key(prompts::POLICY));
    assert!(meta.prompt_hashes.contains_key(prompts::PONYTAIL));
    assert!(meta.prompt_hashes.contains_key(prompts::CAVEMAN));
}

#[tokio::test]
async fn swallows_compress_and_resplices_the_turn_byte_exact() {
    let fixture = fixture().await;
    let original = conversation();

    let (status, response) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(original.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&response).contains("done"));
    assert_eq!(fixture.fake.calls(), 2);

    let first = fixture.fake.body(1);
    let contents = first["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 4);
    assert!(contents[0]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Compressed"));
    assert_eq!(contents[2]["role"], "model");
    assert_eq!(contents[3]["parts"][0]["functionResponse"]["id"], "call_1");
    assert!(
        contents[3]["parts"][0]["functionResponse"]["response"]["result"]
            .as_str()
            .unwrap()
            .starts_with("📦 [acp] compress")
    );

    let expected_call = json!({
        "functionCall": {
            "id": "call_1",
            "name": "compress",
            "args": { "start": 0, "end": 2, "summary": "folded", "topic": "early" }
        },
        "thoughtSignature": "SIG-1"
    });
    let mut next = conversation().as_array().unwrap().clone();
    next.push(json!({ "role": "user", "parts": [{ "text": "four" }] }));
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(Value::Array(next)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fixture.fake.calls(), 3);

    let second = fixture.fake.body(2);
    let contents = second["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 5);
    assert!(contents[0]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Compressed"));
    assert_eq!(contents[1]["parts"][0]["text"], "three");
    assert_eq!(
        serde_json::to_vec(&contents[2]["parts"][0]).unwrap(),
        serde_json::to_vec(&expected_call).unwrap()
    );
    assert_eq!(contents[4]["parts"][0]["text"], "four");
    let function_responses = contents
        .iter()
        .filter(|turn| turn["parts"][0]["functionResponse"].is_object())
        .count();
    assert_eq!(function_responses, 1);
}

#[tokio::test]
async fn sse_proxy_only_turn_is_swallowed_and_the_stream_continues() {
    let final_event = format!(
        "data: {}\n\n",
        json!({
            "response": {
                "candidates": [{"content": {"role": "model", "parts": [{"text": "done"}]}}]
            },
            "usageMetadata": {"promptTokenCount": 21, "candidatesTokenCount": 5}
        })
    );
    let fixture = fixture_sse(vec![
        sse_event(json!([{
            "functionCall": {
                "id": "call_1",
                "name": "compress",
                "args": {"start": 0, "end": 2, "summary": "folded"}
            },
            "thoughtSignature": "SIG-SSE"
        }])),
        final_event,
    ])
    .await;

    let (status, response) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&response);
    assert!(text.contains("done"));
    assert!(!text.contains("functionCall"));
    assert_eq!(fixture.fake.calls(), 2);

    let second = fixture.fake.body(1);
    let contents = second["contents"].as_array().unwrap();
    let call = &contents[contents.len() - 2]["parts"][0];
    assert_eq!(call["functionCall"]["id"], "call_1");
    assert_eq!(call["thoughtSignature"], "SIG-SSE");
    assert_eq!(
        contents[contents.len() - 1]["parts"][0]["functionResponse"]["id"],
        "call_1"
    );
}

#[tokio::test]
async fn sse_passthrough_keeps_every_event_delimited_and_in_order() {
    let upstream: String = ["one", "two", "three"]
        .iter()
        .map(|text| sse_event(json!([{ "text": text }])))
        .collect();
    let fixture = fixture_sse(vec![upstream.clone()]).await;

    let (status, response) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(conversation()),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(String::from_utf8_lossy(&response), upstream);
}

#[tokio::test]
async fn sse_final_event_without_a_blank_line_is_still_delivered_and_metered() {
    let upstream = format!(
        "{}data: {}",
        sse_event(json!([{ "text": "one" }])),
        json!({
            "response": {"candidates": [{"content": {"role": "model", "parts": [{"text": "two"}]}}]},
            "usageMetadata": {"promptTokenCount": 9, "candidatesTokenCount": 2}
        })
    );
    let fixture = fixture_sse(vec![upstream]).await;

    let (_, response) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(conversation()),
    )
    .await;

    let text = String::from_utf8_lossy(&response);
    assert!(text.contains("one") && text.contains("two"));
}

/// The model signs the first call of a step. When that call is a swallowed
/// proxy call, the signature must move to the first call the harness keeps, or
/// the next request carries an unsigned function call and upstream rejects it.
#[tokio::test]
async fn sse_mixed_turn_passes_the_swallowed_signature_to_the_first_kept_call() {
    let mixed = sse_event(json!([
        {
            "functionCall": {
                "id": "p1",
                "name": "compress",
                "args": {"start": 0, "end": 2, "summary": "folded"}
            },
            "thoughtSignature": "P-SIG"
        },
        {"functionCall": {"id": "h1", "name": "bash", "args": {"command": "ls"}}},
        {"functionCall": {"id": "h2", "name": "bash", "args": {"command": "pwd"}}}
    ]));
    let fixture = fixture_sse(vec![mixed]).await;

    let (_, response) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(conversation()),
    )
    .await;

    let text = String::from_utf8_lossy(&response);
    let json = text.trim().strip_prefix("data:").unwrap().trim();
    let payload: Value = serde_json::from_str(json).unwrap();
    let parts = payload["response"]["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["functionCall"]["id"], "h1");
    assert_eq!(parts[0]["thoughtSignature"], "P-SIG");
    assert!(parts[1].get("thoughtSignature").is_none());
}

/// Only the first call of a step is signed. A swallowed call that comes second
/// is stored as a model turn of its own, where it counts as a first call, so
/// the store must give it the signature of its step.
#[tokio::test]
async fn a_swallowed_second_call_is_stored_with_the_signature_of_its_step() {
    let mixed = sse_event(json!([
        {
            "functionCall": {"id": "h1", "name": "bash", "args": {"command": "ls"}},
            "thoughtSignature": "STEP-SIG"
        },
        {
            "functionCall": {
                "id": "p1",
                "name": "compress",
                "args": {"start": 0, "end": 2, "summary": "folded"}
            }
        }
    ]));
    let fixture = fixture_sse(vec![mixed]).await;

    let (_, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(conversation()),
    )
    .await;

    let splices =
        std::fs::read_to_string(fixture.state.inner.data_root.session_splices_file("ses_1"))
            .unwrap();
    let splice: Value = serde_json::from_str(splices.lines().next().unwrap()).unwrap();
    assert_eq!(
        splice["turns"][0]["parts"][0]["thoughtSignature"],
        "STEP-SIG"
    );
}

#[tokio::test]
async fn sse_mixed_turn_keeps_harness_calls_and_splices_the_proxy_pair() {
    let mixed = sse_event(json!([
        {
            "functionCall": {
                "id": "p1",
                "name": "compress",
                "args": {"start": 0, "end": 2, "summary": "folded"}
            },
            "thoughtSignature": "P-SIG"
        },
        {"functionCall": {"id": "h1", "name": "bash", "args": {"command": "ls"}}}
    ]));
    let fixture = fixture_sse(vec![mixed]).await;

    let (status, response) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&response);
    assert!(text.contains("\"h1\""));
    assert!(!text.contains("\"p1\""));
    assert_eq!(fixture.fake.calls(), 1);

    let splices =
        std::fs::read_to_string(fixture.state.inner.data_root.session_splices_file("ses_1"))
            .unwrap();
    assert!(splices.contains("P-SIG"));

    let next = conversation().as_array().unwrap().clone();
    let mut contents = next;
    contents.push(json!({
        "role": "user",
        "parts": [{"functionResponse": {"name": "bash", "id": "h1", "response": {"output": "ok"}}}]
    }));
    contents.push(json!({"role": "user", "parts": [{"text": "next"}]}));
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(Value::Array(contents)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fixture.fake.calls(), 2);

    let second = fixture.fake.body(1);
    let contents = second["contents"].as_array().unwrap();
    let anchor = contents
        .iter()
        .position(|turn| turn["parts"][0]["functionResponse"]["id"] == "h1")
        .unwrap();
    assert_eq!(contents[anchor + 1]["parts"][0]["functionCall"]["id"], "p1");
    assert_eq!(
        contents[anchor + 2]["parts"][0]["functionResponse"]["id"],
        "p1"
    );
}

#[tokio::test]
async fn sse_proxy_only_loop_stops_at_the_round_cap_with_an_error_event() {
    let responses = (0..MAX_TOOL_ROUNDS)
        .map(|index| {
            sse_event(json!([{
                "functionCall": {
                    "id": format!("call_{index}"),
                    "name": "compress",
                    "args": {"start": 0, "end": 1, "summary": format!("fold {index}")}
                }
            }]))
        })
        .collect();
    let fixture = fixture_sse(responses).await;

    let (status, response) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&response);
    assert!(text.contains("round limit"));
    assert_eq!(fixture.fake.calls(), MAX_TOOL_ROUNDS);
}

#[tokio::test]
async fn client_disconnect_drops_the_upstream_stream() {
    let dropped = Arc::new(AtomicBool::new(false));
    let fixture = fixture_with(FakeUpstream::hanging(dropped.clone())).await;
    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "/proxy/{}/v1internal:streamGenerateContent",
            fixture.token
        ))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body(conversation()).to_string()))
        .unwrap();
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    drop(response);

    for _ in 0..100 {
        if dropped.load(Ordering::SeqCst) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("upstream stream was not dropped after the client disconnected");
}

#[tokio::test]
async fn a_silent_upstream_round_times_out_with_an_error_event() {
    let dropped = Arc::new(AtomicBool::new(false));
    let fixture = fixture_config(FakeUpstream::hanging(dropped), |config| {
        config.with_round_timeout(Duration::from_millis(100))
    })
    .await;
    let (status, response) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&response).contains("timed out"));
}

/// `tool_conversation` plus a later user prompt: the latest user message is protected, so a fold
/// that holds a steer needs a user turn after it.
fn tool_conversation_with_later_prompt() -> Value {
    let mut turns = tool_conversation().as_array().unwrap().clone();
    turns.push(json!({ "role": "model", "parts": [{ "text": "ok" }] }));
    turns.push(json!({ "role": "model", "parts": [{ "text": "more" }] }));
    turns.push(json!({ "role": "user", "parts": [{ "text": "later" }] }));
    Value::Array(turns)
}

fn tool_conversation() -> Value {
    json!([
        { "role": "user", "parts": [{ "text": "run it" }] },
        {
            "role": "model",
            "parts": [{ "functionCall": { "name": "bash", "id": "call_9", "args": {} } }]
        },
        {
            "role": "model",
            "parts": [{
                "functionResponse": { "name": "bash", "id": "call_9", "response": { "output": "ok" } }
            }]
        }
    ])
}

/// The steer must be a user turn of its own: agy sends tool results under
/// role `model`, and text in that turn reads as an assistant prefill upstream.
fn steer_part(body: &Value) -> Option<Value> {
    let contents = body["contents"].as_array()?;
    for turn in contents {
        let parts = turn["parts"].as_array()?;
        if parts
            .iter()
            .any(|part| part.get("functionResponse").is_some())
        {
            assert!(
                parts.iter().all(|part| part.get("text").is_none()),
                "no text may ride in a tool-result turn"
            );
        }
    }
    let turn = contents.iter().find(|turn| {
        turn["parts"].as_array().is_some_and(|parts| {
            parts
                .iter()
                .any(|part| part["text"].as_str().is_some_and(|t| t.contains("steer")))
        })
    })?;
    assert_eq!(turn["role"], "user");
    turn["parts"].as_array()?.first().cloned()
}

#[tokio::test]
async fn a_pending_steer_follows_the_tool_result_as_a_user_turn_and_replays_byte_exact() {
    let fixture = fixture().await;
    let queue = fixture.steers.queue("ses_1");
    queue
        .enqueue(
            emberloop_core::steer::SteerKind::Steer,
            "steer: check the tests",
        )
        .await;

    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(tool_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let first = fixture.fake.body(0);
    let part = steer_part(&first).expect("steer part in the tool-result turn");
    assert_eq!(part["text"], "steer: check the tests");

    let entries = queue.entries().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].state,
        emberloop_core::steer::SteerState::Delivered
    );
    assert_eq!(entries[0].route.as_deref(), Some("injected"));

    // The harness drops the injected part next round; the stored splice puts
    // the same part back, unchanged.
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(tool_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let later = fixture.fake.body(2);
    let replayed = steer_part(&later).expect("replayed steer part");
    assert_eq!(part, replayed);
}

#[tokio::test]
async fn a_failed_request_requeues_the_steer() {
    let fixture = fixture().await;
    let queue = fixture.steers.queue("ses_1");
    queue
        .enqueue(emberloop_core::steer::SteerKind::Steer, "steer: retry me")
        .await;

    let (status, _) = send(
        &fixture.state,
        "not-a-token",
        "v1internal:generateContent",
        body(tool_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let entries = queue.entries().await;
    assert_eq!(entries[0].state, emberloop_core::steer::SteerState::Pending);
}

/// What the fake pi upstream saw: (path, auth header, body).
type Seen = Arc<Mutex<Vec<(String, Option<String>, String)>>>;

async fn pi_chat(
    AxumState(seen): AxumState<Seen>,
    headers: axum::http::HeaderMap,
    body: String,
) -> Response {
    let key = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    seen.lock().unwrap().push(("chat".into(), key, body));
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
             data: {\"usage\":{\"prompt_tokens\":44,\"completion_tokens\":2}}\n\n\
             data: [DONE]\n\n",
        ))
        .unwrap()
}

fn pi_upstream(seen: Seen) -> AxumRouter {
    AxumRouter::new()
        .route("/inference/go/openai/v1/chat/completions", post(pi_chat))
        .with_state(seen)
}

#[tokio::test]
async fn pi_chat_requests_inject_prompts_tools_and_steers_and_are_metered() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = pi_upstream(seen.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_opencode_upstream(format!("http://{addr}/inference/go"))
    })
    .await;
    let state = fixture.state.clone();
    let mut events = fixture.events.subscribe();
    fixture
        .steers
        .queue("ses_1")
        .enqueue(
            emberloop_core::steer::SteerKind::Steer,
            "steer: use the fast path",
        )
        .await;

    let req_body = json!({
        "model": "glm-5",
        "stream": true,
        "messages": [
            {"role": "system", "content": "Base system"},
            {"role": "user", "content": "read file"},
            {"role": "assistant", "tool_calls": [{"id": "call_pi_1", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}}]},
            {"role": "tool", "tool_call_id": "call_pi_1", "content": "file body"}
        ]
    });
    let response = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/proxy/{}/v1/chat/completions", fixture.token))
                .header(header::AUTHORIZATION, "Bearer sk-pi")
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("\"content\":\"hi\""));

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1.as_deref(), Some("Bearer sk-pi"));
    let sent: Value = serde_json::from_str(&seen[0].2).unwrap();
    let sys = sent["messages"][0]["content"].as_str().unwrap();
    assert!(sys.contains("Base system"));
    assert!(sys.contains("Emberloop proxy policy"));
    assert!(sys.contains("Ponytail"));
    let tool_names: Vec<&str> = sent["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(tool_names.contains(&"compress"));
    assert!(tool_names.contains(&"decompress"));
    assert!(tool_names.contains(&"search_context"));
    let last_msg = sent["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last_msg["role"], "user");
    assert_eq!(last_msg["content"], "steer: use the fast path");

    let mut saw_usage = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == EventKind::Usage {
            assert_eq!(event.data["inputTokens"], 44);
            saw_usage = true;
        }
    }
    assert!(saw_usage);
}

/// An Anthropic response reports its prompt as `input_tokens` plus cache reads and writes; the
/// published context is the whole prompt, with the reads and writes also named on their own.
#[tokio::test]
async fn anthropic_usage_publishes_the_whole_prompt_and_its_cache_split() {
    async fn messages() -> Response {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":2000,"cache_read_input_tokens":80000,"cache_creation_input_tokens":5000,"output_tokens":9}}"#,
            ))
            .unwrap()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = AxumRouter::new().route("/inference/go/anthropic/v1/messages", post(messages));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_opencode_upstream(format!("http://{addr}/inference/go"))
    })
    .await;
    let mut events = fixture.events.subscribe();
    let body = json!({"model":"claude-sonnet","messages":[{"role":"user","content":"hello"}]});
    let response = router(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/proxy/{}/v1/v1/messages", fixture.token))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut usage = None;
    while let Ok(event) = events.try_recv() {
        if event.kind == EventKind::Usage {
            usage = Some(event.data);
        }
    }
    let usage = usage.expect("a usage event");
    assert_eq!(usage["inputTokens"], 87000);
    assert_eq!(usage["cachedTokens"], 80000);
    assert_eq!(usage["cacheCreationTokens"], 5000);
    assert_eq!(usage["outputTokens"], 9);
}

#[tokio::test]
async fn a_key_provider_route_forwards_to_its_origin_with_the_clients_key() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = AxumRouter::new()
        .route("/openai/v1/chat/completions", post(pi_chat))
        .with_state(seen.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_provider_origin(format!("http://{addr}"))
    })
    .await;
    let call = |path: &'static str| {
        let request = Request::builder()
            .method("POST")
            .uri(format!("/proxy/{}/{path}", fixture.token))
            .header(header::AUTHORIZATION, "Bearer sk-groq")
            .body(Body::from(
                json!({"model": "m", "stream": true, "messages": [{"role": "user", "content": "hi"}]})
                    .to_string(),
            ))
            .unwrap();
        router(fixture.state.clone()).oneshot(request)
    };

    let response = call("x/groq/openai/v1/chat/completions").await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1.as_deref(), Some("Bearer sk-groq"));
        assert!(seen[0].2.contains("Emberloop proxy policy"));
    }

    let unknown = call("x/nope/v1/chat/completions").await.unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_large_tool_result_goes_upstream_as_a_ctx_handle_with_stable_bytes() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = pi_upstream(seen.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_opencode_upstream(format!("http://{addr}/inference/go"))
    })
    .await;
    let big = "ignored target file line\n".repeat(1200);
    let req_body = json!({
        "model": "glm-5",
        "stream": true,
        "messages": [
            {"role": "user", "content": "read it"},
            {"role": "assistant", "tool_calls": [{"id": "call_pi_big", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"target.txt\"}"}}]},
            {"role": "tool", "tool_call_id": "call_pi_big", "content": big}
        ]
    });
    for _ in 0..2 {
        let response = router(fixture.state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/proxy/{}/v1/chat/completions", fixture.token))
                    .header(header::AUTHORIZATION, "Bearer sk-pi")
                    .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
    }

    let seen = seen.lock().unwrap();
    let messages: Vec<Value> = seen
        .iter()
        .map(|(_, _, body)| {
            let sent: Value = serde_json::from_str(body).unwrap();
            sent["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|msg| msg["role"] == "tool")
                .unwrap()
                .clone()
        })
        .collect();
    let text = messages[0]["content"].as_str().unwrap();
    assert!(text.starts_with("[ctx:"), "{text}");
    assert!(text.contains("target.txt"), "{text}");
    // Replace-once: the second request sends the very same bytes.
    assert_eq!(messages[0]["content"], messages[1]["content"]);

    let handle = text
        .split("[ctx:")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .unwrap();
    let spill = fixture.data_root.session_spill_dir("ses_1").join(handle);
    assert_eq!(std::fs::read_to_string(spill).unwrap(), big);
}

#[tokio::test]
async fn pi_anthropic_and_responses_wires_inject_prompts_tools_steers_and_compress() {
    type SeenJson = Arc<Mutex<Vec<(String, Value)>>>;
    let seen: SeenJson = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    async fn messages(AxumState(seen): AxumState<SeenJson>, Json(body): Json<Value>) -> Response {
        let round = seen.lock().unwrap().len();
        seen.lock().unwrap().push(("messages".into(), body));
        if round == 0 {
            Json(json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_comp_1",
                    "name": "compress",
                    "input": {"start": 0, "end": 2, "summary": "folded intro", "topic": "intro"}
                }],
                "usage": {"input_tokens": 20, "output_tokens": 5}
            }))
            .into_response()
        } else {
            Json(json!({
                "id": "msg_2",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "done after compress"}],
                "usage": {"input_tokens": 12, "output_tokens": 4}
            }))
            .into_response()
        }
    }
    async fn responses(AxumState(seen): AxumState<SeenJson>, Json(body): Json<Value>) -> Response {
        seen.lock().unwrap().push(("responses".into(), body));
        Json(json!({
            "id": "resp_1",
            "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "ok"}]}],
            "usage": {"input_tokens": 19, "output_tokens": 3}
        }))
        .into_response()
    }
    let app = AxumRouter::new()
        .route("/inference/go/anthropic/v1/messages", post(messages))
        .route("/inference/go/openai/v1/responses", post(responses))
        .with_state(seen_clone);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_opencode_upstream(format!("http://{addr}/inference/go"))
    })
    .await;
    let mut events = fixture.events.subscribe();
    fixture
        .steers
        .queue("ses_1")
        .enqueue(
            emberloop_core::steer::SteerKind::Steer,
            "steer: anthropic note",
        )
        .await;

    let anthropic_req = json!({
        "model": "claude-sonnet-4-6",
        "messages": [
            {"role": "user", "content": "turn 0"},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "read", "input": {"path": "x"}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "x body"}]}
        ]
    });
    let resp = router(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/proxy/{}/v1/v1/messages", fixture.token))
                .body(Body::from(serde_json::to_vec(&anthropic_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("done after compress"));

    {
        let captured = seen.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let first = &captured[0].1;
        let sys_str = serde_json::to_string(&first["system"]).unwrap();
        assert!(sys_str.contains("Emberloop proxy policy"));
        let msg_str = serde_json::to_string(&first["messages"]).unwrap();
        assert!(msg_str.contains("steer: anthropic note"));
        let second = &captured[1].1;
        let second_msgs = serde_json::to_string(&second["messages"]).unwrap();
        assert!(second_msgs.contains("Compressed turns 0–1 into block b1"));
    }

    let mut saw_compression = false;
    while let Ok(ev) = events.try_recv() {
        if ev.kind == EventKind::Compression {
            saw_compression = true;
        }
    }
    assert!(saw_compression, "expected Compression event on EventBus");

    fixture
        .steers
        .queue("ses_1")
        .enqueue(
            emberloop_core::steer::SteerKind::Steer,
            "steer: responses note",
        )
        .await;
    let responses_req = json!({
        "model": "gpt-5",
        "input": [
            {"role": "user", "content": "hi"},
            {"type": "function_call", "call_id": "call_r1", "name": "read", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call_r1", "output": "ok"}
        ]
    });
    let resp2 = router(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/proxy/{}/v1/responses", fixture.token))
                .body(Body::from(serde_json::to_vec(&responses_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let captured = seen.lock().unwrap();
    let third = &captured[2].1;
    assert!(third["instructions"]
        .as_str()
        .unwrap()
        .contains("Emberloop proxy policy"));
    let input_str = serde_json::to_string(&third["input"]).unwrap();
    assert!(input_str.contains("steer: responses note"));
}

#[test]
fn only_the_pi_wire_suffixes_map_to_opencode_paths() {
    assert_eq!(
        pi_upstream_path("v1/chat/completions").as_deref(),
        Some("openai/v1/chat/completions")
    );
    assert_eq!(
        pi_upstream_path("v1/responses").as_deref(),
        Some("openai/v1/responses")
    );
    assert_eq!(
        pi_upstream_path("v1/v1/messages").as_deref(),
        Some("anthropic/v1/messages")
    );
    assert_eq!(
        pi_upstream_path("v1/models/gemini-4-argon:streamGenerateContent").as_deref(),
        Some("google/v1beta/models/gemini-4-argon:streamGenerateContent")
    );
    assert_eq!(pi_upstream_path("v1internal:generateContent"), None);
    assert_eq!(pi_upstream_path("v1internal/models"), None);
}

#[tokio::test]
async fn openai_chat_streaming_proxy_tool_is_swallowed_and_followed_up() {
    type SeenJson = Arc<Mutex<Vec<Value>>>;
    let seen: SeenJson = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    async fn chat(AxumState(seen): AxumState<SeenJson>, Json(body): Json<Value>) -> Response {
        let round = {
            let mut seen = seen.lock().unwrap();
            let round = seen.len();
            seen.push(body);
            round
        };
        let body = if round == 0 {
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_c1\",\"type\":\"function\",\"function\":{\"name\":\"compress\",\"arguments\":\"{\\\"start\\\":0,\\\"end\\\":1,\"}}]}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"summary\\\":\\\"folded intro\\\"}\"}}]}}]}\n\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
             data: [DONE]\n\n"
                .to_string()
        } else {
            "data: {\"choices\":[{\"delta\":{\"content\":\"done after compress\"}}]}\n\n\
             data: [DONE]\n\n"
                .to_string()
        };
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }
    let app = AxumRouter::new()
        .route("/inference/go/openai/v1/chat/completions", post(chat))
        .with_state(seen_clone);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_opencode_upstream(format!("http://{addr}/inference/go"))
    })
    .await;

    let req_body = json!({
        "model": "glm-5",
        "stream": true,
        "messages": [
            {"role": "system", "content": "Base"},
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"},
            {"role": "user", "content": "more"},
            {"role": "assistant", "content": "still"},
            {"role": "user", "content": "latest"}
        ]
    });
    let response = router(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/proxy/{}/v1/chat/completions", fixture.token))
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("done after compress"), "{text}");
    assert!(!text.contains("tool_calls"), "proxy call leaked: {text}");
    assert!(!text.contains("\"compress\""), "proxy name leaked: {text}");

    let captured = seen.lock().unwrap();
    assert_eq!(captured.len(), 2);
    let second = serde_json::to_string(&captured[1]).unwrap();
    assert!(
        second.contains("Compressed turns 0–0 into block b1"),
        "{second}"
    );
}

#[tokio::test]
async fn openai_streaming_publishes_usage_even_when_the_client_stops_reading() {
    type SeenJson = Arc<Mutex<Vec<Value>>>;
    let seen: SeenJson = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    async fn chat(AxumState(seen): AxumState<SeenJson>, Json(body): Json<Value>) -> Response {
        let round = {
            let mut seen = seen.lock().unwrap();
            let round = seen.len();
            seen.push(body);
            round
        };
        let body = if round == 0 {
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_u1\",\"type\":\"function\",\"function\":{\"name\":\"compress\",\"arguments\":\"{\\\"start\\\":0,\\\"end\\\":1,\\\"summary\\\":\\\"folded intro\\\"}\"}}]}}]}\n\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":77,\"completion_tokens\":3}}\n\n\
             data: [DONE]\n\n"
                .to_string()
        } else {
            "data: {\"choices\":[{\"delta\":{\"content\":\"done after compress\"}}]}\n\n\
             data: {\"choices\":[],\"usage\":{\"prompt_tokens\":77,\"completion_tokens\":3}}\n\n\
             data: [DONE]\n\n"
                .to_string()
        };
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }
    let app = AxumRouter::new()
        .route("/inference/go/openai/v1/chat/completions", post(chat))
        .with_state(seen_clone);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_opencode_upstream(format!("http://{addr}/inference/go"))
    })
    .await;
    let mut events = fixture.events.subscribe();

    let req_body = json!({
        "model": "glm-5",
        "stream": true,
        "messages": [
            {"role": "system", "content": "Base"},
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"},
            {"role": "user", "content": "more"},
            {"role": "assistant", "content": "still"},
            {"role": "user", "content": "latest"}
        ]
    });
    let response = router(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/proxy/{}/v1/chat/completions", fixture.token))
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // A real harness stops reading once it has the answer, so the usage event that
    // trails the text may never be turned into an event by the reader.
    let mut stream = response.into_body().into_data_stream();
    let mut text = String::new();
    while let Some(Ok(chunk)) = futures::StreamExt::next(&mut stream).await {
        text.push_str(&String::from_utf8_lossy(&chunk));
        if text.contains("done after compress") {
            break;
        }
    }
    drop(stream);
    let mut tokens = Vec::new();
    let mut compressed = 0;
    while let Ok(event) = events.try_recv() {
        if event.kind == EventKind::Usage {
            tokens.push(event.data["inputTokens"].as_u64().unwrap());
            compressed = event.data["breakdown"]["compression"].as_u64().unwrap();
        }
    }
    assert!(
        compressed > 0,
        "the follow-up must refresh its breakdown even with identical provider usage"
    );
    assert_eq!(tokens.last(), Some(&77), "usage events: {tokens:?}");
}

#[tokio::test]
async fn anthropic_streaming_proxy_tool_is_swallowed_and_followed_up() {
    type SeenJson = Arc<Mutex<Vec<Value>>>;
    let seen: SeenJson = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    async fn messages(AxumState(seen): AxumState<SeenJson>, Json(body): Json<Value>) -> Response {
        let round = {
            let mut seen = seen.lock().unwrap();
            let round = seen.len();
            seen.push(body);
            round
        };
        let body = if round == 0 {
            "event: message_start\n\
             data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"role\":\"assistant\"}}\n\n\
             event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_c1\",\"name\":\"compress\",\"input\":{}}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"start\\\":0,\\\"end\\\":1,\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"summary\\\":\\\"folded intro\\\"}\"}}\n\n\
             event: content_block_stop\n\
             data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
             event: message_delta\n\
             data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n\
             event: message_stop\n\
             data: {\"type\":\"message_stop\"}\n\n"
                .to_string()
        } else {
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"done after compress\"}}\n\n\
             data: {\"type\":\"message_stop\"}\n\n"
                .to_string()
        };
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }
    let app = AxumRouter::new()
        .route("/inference/go/anthropic/v1/messages", post(messages))
        .with_state(seen_clone);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_opencode_upstream(format!("http://{addr}/inference/go"))
    })
    .await;

    let req_body = json!({
        "model": "claude-sonnet",
        "stream": true,
        "messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"},
            {"role": "user", "content": "more"},
            {"role": "assistant", "content": "still"},
            {"role": "user", "content": "latest"}
        ]
    });
    let response = router(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/proxy/{}/v1/v1/messages", fixture.token))
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("done after compress"), "{text}");
    assert!(!text.contains("tool_use"), "proxy call leaked: {text}");
    assert!(!text.contains("\"compress\""), "proxy name leaked: {text}");

    let captured = seen.lock().unwrap();
    assert_eq!(captured.len(), 2);
    let second = serde_json::to_string(&captured[1]).unwrap();
    assert!(
        second.contains("Compressed turns 0–0 into block b1"),
        "{second}"
    );
}

/// A fold chosen while a steer sat inside the history must still apply to the history the harness
/// resends, which does not carry the steer turn: the steer is put back first, so the fold's stored
/// turns match again.
#[tokio::test]
async fn a_fold_over_a_spliced_steer_still_applies_to_the_resent_history() {
    let final_event = format!(
        "data: {}\n\n",
        json!({
            "response": {
                "candidates": [{"content": {"role": "model", "parts": [{"text": "done"}]}}]
            },
            "usageMetadata": {"promptTokenCount": 21, "candidatesTokenCount": 5}
        })
    );
    let fixture = fixture_sse(vec![
        sse_event(json!([{
            "functionCall": {
                "id": "call_fold",
                "name": "compress",
                "args": {"start": 0, "end": 4, "summary": "folded", "topic": "all"}
            },
            "thoughtSignature": "SIG-FOLD"
        }])),
        final_event,
    ])
    .await;
    fixture
        .steers
        .queue("ses_1")
        .enqueue(
            emberloop_core::steer::SteerKind::Steer,
            "steer: check the tests",
        )
        .await;

    // Turn one: the steer is injected after the tool result, so the model folds four turns,
    // the steer among them.
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(tool_conversation_with_later_prompt()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Turn two: the harness resends its own history, without the steer or the proxy's pair.
    let mut next = tool_conversation_with_later_prompt()
        .as_array()
        .unwrap()
        .clone();
    next.push(json!({ "role": "user", "parts": [{ "text": "next" }] }));
    let before = fixture.fake.calls();
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(Value::Array(next)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let sent = fixture.fake.body(before);
    let wire = sent["contents"].to_string();
    assert!(
        !wire.contains("run it"),
        "the folded turns are still on the wire: {wire}"
    );
    assert!(wire.contains("Compressed"), "{wire}");
    assert!(wire.contains("next"), "{wire}");
}

/// A six-turn conversation whose tool result is large enough (about 80000 tokens at
/// chars / 4) to pass the 75000-token first-nudge gate of a 1M window. Turns 1 and 2
/// are the compressible range (the last three turns are kept).
fn heavy_conversation() -> Value {
    json!([
        { "role": "user", "parts": [{ "text": "run it" }] },
        {
            "role": "model",
            "parts": [{ "functionCall": { "name": "bash", "id": "call_9", "args": {} } }]
        },
        {
            "role": "model",
            "parts": [{
                "functionResponse": { "name": "bash", "id": "call_9", "response": { "output": "x".repeat(320_000) } }
            }]
        },
        { "role": "user", "parts": [{ "text": "next" }] },
        { "role": "model", "parts": [{ "text": "ok" }] },
        { "role": "user", "parts": [{ "text": "continue" }] }
    ])
}

fn nudge_parts(body: &Value) -> Vec<Value> {
    body["contents"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|turn| turn["parts"].as_array().cloned().unwrap_or_default())
        .filter(|part| {
            part["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("This is an efficiency nudge"))
        })
        .collect()
}

fn nudge_turn_follows_tool_result(body: &Value) -> bool {
    let contents = body["contents"].as_array().expect("contents");
    let result = contents
        .iter()
        .position(|turn| {
            turn["parts"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|p| p.get("functionResponse").is_some()))
        })
        .expect("tool result turn");
    contents[result + 1]["parts"]
        .as_array()
        .is_some_and(|parts| {
            parts.iter().any(|p| {
                p["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("This is an efficiency nudge"))
            })
        })
}

/// A fake whose every call answers plain text, so no proxy tool round runs and each client request is one upstream call.
fn text_only_fake() -> FakeUpstream {
    let fake = FakeUpstream::new();
    fake.calls.store(1, Ordering::SeqCst);
    fake
}

fn nudge_text(body: &Value) -> Option<String> {
    nudge_parts(body)
        .first()
        .and_then(|part| part["text"].as_str().map(str::to_string))
}

fn splices_file_text(fixture: &Fixture) -> String {
    std::fs::read_to_string(fixture.data_root.session_splices_file("ses_1")).unwrap_or_default()
}

#[tokio::test]
async fn a_first_nudge_is_transient_and_rides_the_tool_result_turn() {
    let fixture = fixture_with(text_only_fake()).await;
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(heavy_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let first = fixture.fake.body(0);
    assert_eq!(
        nudge_parts(&first).len(),
        1,
        "one nudge on the request where the gate fires"
    );
    assert!(
        nudge_turn_follows_tool_result(&first),
        "the nudge is a user turn right after the tool result"
    );
    assert!(
        !splices_file_text(&fixture).contains("nudge"),
        "no splice stores a nudge"
    );

    // The gate does not fire again on the next request, so the nudge is not resent.
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(heavy_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        nudge_parts(&fixture.fake.body(1)).is_empty(),
        "a transient nudge is absent from the next request"
    );
    assert!(!splices_file_text(&fixture).contains("nudge"));
}

#[tokio::test]
async fn a_compress_drops_the_stale_usage_so_the_next_request_does_not_renudge() {
    let fixture = fixture().await;
    let runtime = fixture.state.runtime("ses_1").await.unwrap();
    runtime.last_input_tokens.store(80_000, Ordering::Relaxed);
    let mut contents = conversation().as_array().unwrap().clone();
    let call = ProxyCall {
        name: "compress".into(),
        id: "call_c".into(),
        part: json!({}),
        args: json!({"start": 0, "end": 2, "summary": "folded"}),
    };

    execute_tool(
        &runtime,
        &mut contents,
        &call,
        &api::state::Settings {
            preserve_recent_messages: 0,
            ..api::state::Settings::default()
        },
        64_000,
    )
    .unwrap();

    assert_eq!(
        runtime.last_input_tokens.load(Ordering::Relaxed),
        0,
        "tokens from before the fold must not reopen the nudge gate"
    );
}

#[tokio::test]
async fn a_failed_upstream_request_does_not_spend_the_first_nudge() {
    let fake = text_only_fake();
    fake.fail_next.store(true, Ordering::SeqCst);
    let fixture = fixture_with(fake).await;
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(heavy_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(nudge_text(&fixture.fake.body(0)).is_some());

    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(heavy_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        nudge_text(&fixture.fake.body(1)).is_some(),
        "the model never saw the failed request's nudge, so the first nudge fires again"
    );
}

#[tokio::test]
async fn a_cancelled_upstream_request_does_not_spend_the_first_nudge() {
    let fake = text_only_fake();
    fake.stall.store(true, Ordering::SeqCst);
    let fixture = fixture_with(fake.clone()).await;
    let state = fixture.state.clone();
    let token = fixture.token.clone();
    let request = tokio::spawn(async move {
        send(
            &state,
            &token,
            "v1internal:generateContent",
            body(heavy_conversation()),
        )
        .await
    });
    // The upstream has the request, so prepare ran and the request is in flight.
    fake.arrived.notified().await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());

    fake.stall.store(false, Ordering::SeqCst);
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(heavy_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        nudge_text(&fixture.fake.body(0)).is_some(),
        "the cancelled request gave the model no answer, so the nudge fires on the retry"
    );
}

#[tokio::test]
async fn nudge_turn_numbers_match_the_final_contents_after_a_stored_steer() {
    let fixture = fixture().await;
    fixture.settings.write().await.preserve_recent_messages = 3;
    let runtime = fixture.state.runtime("ses_1").await.unwrap();
    lock(&runtime.splices)
        .unwrap()
        .insert(Splice {
            id: "steer-drift".into(),
            tool_result_id: "call_9".into(),
            marker: String::new(),
            turns: vec![json!({ "role": "user", "parts": [{ "text": "steer: keep the tests" }] })],
            append_parts: Vec::new(),
            after_turn: None,
        })
        .unwrap();

    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(heavy_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sent = fixture.fake.body(0);
    let text = nudge_text(&sent).expect("nudge text");

    // Final contents without the nudge: the six harness turns plus the stored steer.
    // compress takes start inclusive and end exclusive; the kept tail is the last 3 turns.
    let final_turns = sent["contents"].as_array().unwrap().len() - 1;
    let end = final_turns - 3;
    assert!(text.contains(&format!("turns 1–{}", end - 1)), "{text}");
}

#[tokio::test]
async fn nothing_is_injected_below_the_first_threshold() {
    let fixture = fixture().await;
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(tool_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(nudge_parts(&fixture.fake.body(0)).is_empty());
}

#[tokio::test]
async fn the_nudge_gate_survives_a_runtime_rebuild() {
    let fixture = fixture_with(text_only_fake()).await;
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(heavy_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(nudge_parts(&fixture.fake.body(0)).len(), 1);

    // A fresh ProxyState on the same data root has no runtime cache: the gate must come from nudge.json.
    let rebuilt = ProxyState::new(
        fixture.config.clone(),
        fixture.sessions.clone(),
        fixture.projects.clone(),
        fixture.data_root.clone(),
        fixture.prompts.clone(),
        fixture.models.clone(),
        fixture.events.clone(),
        fixture.steers.clone(),
        fixture.settings.clone(),
    );
    let (status, _) = send(
        &rebuilt,
        &fixture.token,
        "v1internal:generateContent",
        body(heavy_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        nudge_parts(&fixture.fake.body(1)).is_empty(),
        "the rebuilt runtime does not fire another nudge"
    );
    let saved = std::fs::read_to_string(
        fixture
            .data_root
            .session_splices_file("ses_1")
            .with_file_name("nudge.json"),
    )
    .expect("nudge.json written next to splices.jsonl");
    assert!(saved.contains("\"first_fired\":true"), "{saved}");
}

#[tokio::test]
async fn conversation_reset_archives_blocks_without_reloading_or_overwriting_them() {
    let fixture = fixture_with(text_only_fake()).await;
    let spill = fixture.data_root.session_spill_dir("ses_1");
    for generation in 1..=2 {
        let mut compressor = NativeCompressor::new(&spill).unwrap();
        let mut contents = vec![
            json!({"role": "user", "parts": [{"text": "original"}]}),
            json!({"role": "model", "parts": [{"text": "ack"}]}),
            json!({"role": "user", "parts": [{"text": "latest"}]}),
        ];
        compressor
            .compress_batch(
                &mut contents,
                vec![CompressCall {
                    start: 0,
                    end: 1,
                    summary: "saved plan".into(),
                    topic: None,
                }],
                &CompressPolicy::default(),
            )
            .unwrap();
        let bytes = std::fs::read(spill.join("blocks/b1.json")).unwrap();
        fixture.state.reset_history("ses_1").await.unwrap();
        assert_eq!(NativeCompressor::new(&spill).unwrap().status().blocks, 0);
        assert_eq!(
            std::fs::read(spill.join(format!("blocks-reset-{generation}/b1.json"))).unwrap(),
            bytes
        );
    }
}

#[test]
fn compress_args_accept_a_ranges_array_or_the_single_form() {
    let batch = parse_compress_calls(&json!({
        "ranges": [
            {"start": 1, "end": 3, "summary": "a", "topic": "first"},
            {"start": 4, "end": 6, "summary": "b"}
        ]
    }))
    .unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!((batch[0].start, batch[0].end), (1, 3));
    assert_eq!(batch[0].topic.as_deref(), Some("first"));
    assert_eq!(batch[1].topic, None);

    let single =
        parse_compress_calls(&json!({"start": 2, "end": 5, "summary": "s", "topic": "t"})).unwrap();
    assert_eq!(single.len(), 1);
    assert_eq!((single[0].start, single[0].end), (2, 5));
}

#[test]
fn compress_args_name_the_range_that_is_malformed() {
    let error = parse_compress_calls(&json!({
        "ranges": [
            {"start": 1, "end": 3, "summary": "a"},
            {"start": 4, "summary": "b"}
        ]
    }))
    .unwrap_err();
    assert!(error.contains("range 2"), "{error}");
    assert!(error.contains("end"), "{error}");

    assert!(parse_compress_calls(&json!({"ranges": []})).is_err());
    assert!(parse_compress_calls(&json!({"ranges": "nope"})).is_err());
    assert!(parse_compress_calls(&json!({})).is_err());
}

#[tokio::test]
async fn one_compress_call_with_two_ranges_folds_both_on_the_wire() {
    let final_event = format!(
        "data: {}\n\n",
        json!({
            "response": {
                "candidates": [{"content": {"role": "model", "parts": [{"text": "done"}]}}]
            },
            "usageMetadata": {"promptTokenCount": 21, "candidatesTokenCount": 5}
        })
    );
    let fixture = fixture_sse(vec![
        sse_event(json!([{
            "functionCall": {
                "id": "call_batch",
                "name": "compress",
                "args": {"ranges": [
                    {"start": 0, "end": 2, "summary": "first fold", "topic": "a"},
                    {"start": 2, "end": 4, "summary": "second fold", "topic": "b"}
                ]}
            },
            "thoughtSignature": "SIG-B"
        }])),
        final_event,
    ])
    .await;
    let turns: Vec<Value> = (0..6)
        .map(|n| {
            json!({"role": if n % 2 == 0 { "user" } else { "model" },
                   "parts": [{"text": format!("turn {n}")}]})
        })
        .collect();
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:streamGenerateContent",
        body(Value::Array(turns)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let followup = fixture.fake.body(1)["contents"].to_string();
    assert!(
        followup.contains("block b1") && followup.contains("block b2"),
        "{followup}"
    );
    assert!(
        !followup.contains("turn 0") && !followup.contains("turn 3"),
        "{followup}"
    );
    assert!(
        followup.contains("turn 4") && followup.contains("turn 5"),
        "{followup}"
    );
}

#[test]
fn acp_status_reports_usage_blocks_and_ranges() {
    let contents = json!([
        {"role": "user", "parts": [{"text": "one"}]},
        {"role": "model", "parts": [{"text": "two"}]},
        {"role": "model", "parts": [{"text": "three"}]},
        {"role": "user", "parts": [{"text": "four"}]},
        {"role": "model", "parts": [{"text": "five"}]},
        {"role": "user", "parts": [{"text": "six"}]}
    ]);
    let status = crate::compressor::CompressorStatus {
        blocks: 2,
        active_blocks: 1,
        restored_blocks: 1,
        spilled_turns: 3,
        tokens_compressed: 4_000,
        spill_dir: "/tmp/spill".into(),
    };
    let text = acp_status_text(
        contents.as_array().unwrap(),
        10_000,
        &status,
        &api::state::Settings::default(),
    );
    assert!(text.contains("ACP STATUS"), "{text}");
    assert!(text.contains("1 active, 2 total"), "{text}");
    assert!(text.contains("3 stored messages"), "{text}");
    assert!(text.contains("4.0K tokens compressed"), "{text}");
}

#[test]
fn acp_status_numbers_follow_the_reference_golden() {
    let path = format!(
        "{}/parity/golden/window-64k-pressure.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let status = golden["frames"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|frame| frame.get("status"))
        .expect("a golden frame with status");
    let text = format_acp_status(
        status["tokenCount"].as_u64().unwrap(),
        status["modelContextLimit"].as_u64().unwrap() as usize,
        status["activeBlocks"].as_u64().unwrap() as usize,
        status["totalBlocks"].as_u64().unwrap() as usize,
        status["breakdown"]["storedMessages"].as_u64().unwrap() as usize,
        status["tokensCompressed"].as_u64().unwrap(),
        status["breakdown"]["retrievals"].as_u64().unwrap() as usize,
        "ranges",
    );
    assert!(text.contains("0 active, 0 total"), "{text}");
    assert!(text.contains("(1%)"), "{text}");
}

#[test]
fn forged_markers_are_stripped_from_model_text() {
    let mut contents = vec![
        json!({"role": "model", "parts": [{"text": "📦 [acp] compress\nanswer"}]}),
        json!({"role": "user", "parts": [{"text": "📦 [acp] keep me"}]}),
    ];
    strip_forged_markers(&mut contents);
    assert_eq!(contents[0]["parts"][0]["text"], "answer");
    assert_eq!(
        contents[1]["parts"][0]["text"], "📦 [acp] keep me",
        "only model text is cleaned"
    );
}

#[test]
fn emergency_truncation_shortens_tool_output_but_not_a_summary() {
    let big = "x".repeat(10_000);
    let mut contents = vec![
        json!({"role": "user", "parts": [{"text": "📦 [acp] Compressed turns 1–2"}]}),
        json!({"role": "model", "parts": [{"functionResponse": {"id": "c1", "name": "bash", "response": {"result": big}}}]}),
    ];
    truncate_emergency(&mut contents);
    assert_eq!(
        contents[0]["parts"][0]["text"],
        "📦 [acp] Compressed turns 1–2"
    );
    let result = contents[1]["parts"][0]["functionResponse"]["response"]["result"]
        .as_str()
        .unwrap();
    assert!(result.contains("emergency truncation"), "{}", result.len());
    assert!(result.len() < 4_000);
}

#[tokio::test]
async fn preflight_folds_until_the_request_fits_the_window() {
    let fixture = fixture().await;
    let runtime = fixture.state.runtime("ses_1").await.unwrap();
    let mut contents = (0..12)
        .map(|index| json!({"role": "user", "parts": [{"text": format!("turn {index} {}", "x".repeat(400))}]}))
        .collect::<Vec<_>>();
    let before = turns_tokens(&contents);
    let settings = api::state::Settings::default();
    let folded = preflight_fold(&runtime, &mut contents, 20, &settings).unwrap();
    assert!(folded.is_some());
    assert!(turns_tokens(&contents) < before);
    assert!(contents[1]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Preflight fold"));
}

#[tokio::test]
async fn preflight_folds_the_old_turns_and_keeps_the_latest_user_message_verbatim() {
    let fixture = fixture().await;
    let runtime = fixture.state.runtime("ses_1").await.unwrap();
    // The latest user prompt is turn 10, followed by a long model tail: a fold of everything but
    // the preserved tail would reach it.
    let mut contents: Vec<Value> = (0..14)
        .map(|index| match index {
            0 => json!({"role": "user", "parts": [{"text": "first prompt"}]}),
            10 => json!({"role": "user", "parts": [{"text": "latest prompt"}]}),
            _ => json!({"role": "model", "parts": [{"text": format!("step {index} with some text")}]}),
        })
        .collect();
    let settings = api::state::Settings::default();
    let folded = preflight_fold(&runtime, &mut contents, 20, &settings).unwrap();
    assert!(folded.is_some());
    assert!(contents.len() < 14);
    assert!(contents
        .iter()
        .any(|turn| turn["parts"][0]["text"] == "latest prompt"));
}

#[test]
fn the_compressible_range_ends_before_the_latest_user_message() {
    let turns = vec![
        json!({"role": "user", "parts": [{"text": "first prompt"}]}),
        json!({"role": "model", "parts": [{"text": "a".repeat(4_000)}]}),
        json!({"role": "user", "parts": [{"text": "b".repeat(4_000)}]}),
        json!({"role": "model", "parts": [{"text": "c".repeat(4_000)}]}),
        json!({"role": "user", "parts": [{"text": "latest prompt"}]}),
        json!({"role": "model", "parts": [{"text": "d".repeat(160_000)}]}),
        json!({"role": "model", "parts": [{"text": "e".repeat(160_000)}]}),
        json!({"role": "model", "parts": [{"text": "tail"}]}),
    ];
    let view = compressible_view(&turns, 2);
    // The range is turns 1..4: the latest user message at 4 is never part of it, so the
    // 160000-character results after it are not counted as compressible mass.
    assert_eq!(view.tokens, turns_tokens(&turns[1..4]));
    assert!(view.ranges.contains("turns 1–3"), "{}", view.ranges);
}

#[test]
fn the_compressible_range_is_not_cut_when_the_only_user_message_is_the_first_turn() {
    // Turn 0 is the latest user message and no range may include it, but `compress` ranges
    // start at 1, so the whole tail after it is compressible (a long single agent turn).
    let turns = vec![
        json!({"role": "user", "parts": [{"text": "start the task"}]}),
        json!({"role": "model", "parts": [{"text": "a".repeat(40_000)}]}),
        json!({"role": "model", "parts": [{"text": "b".repeat(40_000)}]}),
        json!({"role": "model", "parts": [{"text": "tail"}]}),
    ];
    let view = compressible_view(&turns, 1);
    assert_eq!(view.tokens, turns_tokens(&turns[1..3]));
    assert!(view.ranges.contains("turns 1–2"), "{}", view.ranges);
}

#[tokio::test]
async fn a_nudge_is_silent_when_the_latest_user_message_blocks_the_only_mass() {
    let fixture = fixture().await;
    let runtime = fixture.state.runtime("ses_1").await.unwrap();
    let settings = api::state::Settings {
        preserve_recent_messages: 2,
        ..Default::default()
    };
    // The 300000-character result sits after the latest user message at turn 2. Inside
    // 1..len-2 it would be counted, but no fold may cover the latest message, so the
    // range ends at turn 2 and holds too little mass to carry a nudge.
    let mut contents = vec![
        json!({"role": "user", "parts": [{"text": "first prompt"}]}),
        json!({"role": "model", "parts": [{"text": "a".repeat(2_000)}]}),
        json!({"role": "user", "parts": [{"text": "latest prompt"}]}),
        json!({"role": "model", "parts": [{"functionCall": {"name": "bash", "id": "call_1", "args": {}}}]}),
        json!({"role": "model", "parts": [{"functionResponse": {"name": "bash", "id": "call_1", "response": {"output": "x".repeat(300_000)}}}]}),
        json!({"role": "model", "parts": [{"text": "ok"}]}),
        json!({"role": "model", "parts": [{"text": "more"}]}),
    ];
    let (nudge, gate) = inject_nudge(&runtime, &mut contents, 100_000, &settings).unwrap();
    assert!(nudge.is_none(), "{:?}", nudge.map(|report| report.kind));
    assert!(gate.is_some());
    assert_eq!(contents.len(), 7, "no nudge turn was inserted");

    // Control: the same mass with the latest message at the end is compressible, so a nudge
    // fires (pressure, as the usage is over 75 percent of this window).
    let mut control = vec![
        json!({"role": "user", "parts": [{"text": "first prompt"}]}),
        json!({"role": "model", "parts": [{"text": "a".repeat(2_000)}]}),
        json!({"role": "model", "parts": [{"functionCall": {"name": "bash", "id": "call_1", "args": {}}}]}),
        json!({"role": "model", "parts": [{"functionResponse": {"name": "bash", "id": "call_1", "response": {"output": "x".repeat(300_000)}}}]}),
        json!({"role": "model", "parts": [{"text": "ok"}]}),
        json!({"role": "model", "parts": [{"text": "more"}]}),
        json!({"role": "user", "parts": [{"text": "continue"}]}),
    ];
    let (nudge, _) = inject_nudge(&runtime, &mut control, 100_000, &settings).unwrap();
    assert_eq!(nudge.map(|report| report.kind), Some("pressure"));
}

#[tokio::test]
async fn switches_skip_nudge_and_tool_injection() {
    let fixture = fixture_with(text_only_fake()).await;
    {
        let mut settings = fixture.settings.write().await;
        settings.inject_nudge = false;
        settings.inject_tool = false;
    }
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(heavy_conversation()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sent = fixture.fake.body(0);
    assert!(nudge_parts(&sent).is_empty());
    let tools = sent["tools"].to_string();
    assert!(!tools.contains("compress"), "{tools}");
}

/// The opencode chat wire is what the phone Session runs on, so the nudge and the proxy tools
/// must reach it too: a user message after the tool result, carrying no marker key.
#[tokio::test]
async fn a_chat_wire_nudge_arrives_as_a_user_message_and_the_tools_are_offered() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = pi_upstream(seen.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_opencode_upstream(format!("http://{addr}/inference/go"))
    })
    .await;
    let state = fixture.state.clone();
    let runtime = state.runtime("ses_1").await.unwrap();
    runtime.last_input_tokens.store(80_000, Ordering::Relaxed);

    // Four 8000-character results are 2000 reference tokens each; the old byte rule counted the
    // `_raw` copy too and cleared the minimum compressible mass at 4000.
    let filler = "x".repeat(8_000);
    let mut messages = vec![json!({"role": "system", "content": "Base system"})];
    for i in 0..4 {
        messages.push(json!({"role": "user", "content": format!("read file {i}")}));
        messages.push(json!({"role": "assistant", "tool_calls": [{"id": format!("call_pi_{i}"), "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}}]}));
        messages.push(
            json!({"role": "tool", "tool_call_id": format!("call_pi_{i}"), "content": filler}),
        );
    }
    let req_body = json!({
        "model": "glm-5",
        "stream": true,
        "messages": messages,
    });
    let response = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/proxy/{}/v1/chat/completions", fixture.token))
                .header(header::AUTHORIZATION, "Bearer sk-pi")
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let seen = seen.lock().unwrap();
    let sent: Value = serde_json::from_str(&seen[0].2).unwrap();
    let messages = sent["messages"].as_array().expect("messages");
    let nudge = messages
        .iter()
        .position(|message| {
            message["content"]
                .as_str()
                .is_some_and(|text| text.starts_with("This is an efficiency nudge"))
        })
        .expect("the nudge reached the chat wire");
    assert_eq!(messages[nudge]["role"], "user");
    assert_eq!(
        messages[nudge - 1]["role"],
        "tool",
        "the nudge rides right after the tool result"
    );
    assert!(
        !seen[0].2.contains("_nudge"),
        "no marker key may reach the upstream: {}",
        seen[0].2
    );
    let tools = sent["tools"].to_string();
    assert!(tools.contains("\"compress\""), "{tools}");
}

/// A fold's stored turns carry the `ctx` handle, because the fold is taken after arrival
/// replacement. A later request arrives with the raw result again, so the handle must be put
/// back before the fold is looked for, or the fold applies once and is lost.
#[tokio::test]
async fn a_fold_survives_the_next_request_that_carries_the_raw_result_again() {
    type SeenJson = Arc<Mutex<Vec<Value>>>;
    let seen: SeenJson = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    async fn chat(AxumState(seen): AxumState<SeenJson>, Json(body): Json<Value>) -> Response {
        let round = {
            let mut seen = seen.lock().unwrap();
            let round = seen.len();
            seen.push(body);
            round
        };
        let body = if round == 0 {
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_fold\",\"type\":\"function\",\"function\":{\"name\":\"compress\",\"arguments\":\"{\\\"ranges\\\":[{\\\"start\\\":0,\\\"end\\\":3,\\\"summary\\\":\\\"folded wide\\\"}]}\"}}]}}]}\n\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
             data: [DONE]\n\n"
                .to_string()
        } else {
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
             data: [DONE]\n\n"
                .to_string()
        };
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }
    let app = AxumRouter::new()
        .route("/inference/go/openai/v1/chat/completions", post(chat))
        .with_state(seen_clone);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_opencode_upstream(format!("http://{addr}/inference/go"))
    })
    .await;
    fixture.settings.write().await.preserve_recent_messages = 0;

    let big = "ignored target file line\n".repeat(1200);
    let req_body = json!({
        "model": "glm-5",
        "stream": true,
        "messages": [
            {"role": "system", "content": "Base"},
            {"role": "user", "content": "read it"},
            {"role": "assistant", "tool_calls": [{"id": "call_big", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"target.txt\"}"}}]},
            {"role": "tool", "tool_call_id": "call_big", "content": big},
            {"role": "assistant", "content": "got it"},
            {"role": "assistant", "content": "more"},
            {"role": "user", "content": "now summarise"}
        ]
    });
    for _ in 0..2 {
        let response = router(fixture.state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/proxy/{}/v1/chat/completions", fixture.token))
                    .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
    }

    let seen = seen.lock().unwrap();
    // Round 0 is the first client request, 1 the proxy round after the fold, 2 the second client
    // request rebuilt from the harness' own history.
    assert_eq!(seen.len(), 3, "one proxy round and two client requests");
    let again = serde_json::to_string(&seen[2]).unwrap();
    assert!(
        again.contains("folded wide"),
        "the fold did not come back: {}",
        &again[..again.len().min(800)]
    );
    assert!(
        !again.contains("call_big"),
        "the folded turns are still in the request: {}",
        &again[..again.len().min(800)]
    );
}

#[tokio::test]
async fn large_image_requests_reach_upstream_and_are_not_counted_as_text_tokens() {
    let fixture = fixture().await;
    fixture.fake.calls.store(1, Ordering::SeqCst);
    let mut events = fixture.events.subscribe();
    let contents = json!([{ "role":"user", "parts":[
        {"text":"describe this"},
        {"inlineData":{"mimeType":"image/png", "data":"A".repeat(3 * 1024 * 1024)}}
    ]}]);
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(contents),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let event = events.try_recv().unwrap();
    assert_eq!(event.kind, EventKind::Usage);
    assert_eq!(event.data["breakdown"]["imageCount"], 1);
    assert!(event.data["breakdown"]["estimatedTokens"].as_u64().unwrap() < 10000);
}

#[test]
fn the_proxy_body_limit_fits_five_maximum_size_image_attachments() {
    let encoded_images = 5 * (10 * 1024 * 1024usize).div_ceil(3) * 4;
    assert!(MAX_BODY_BYTES >= encoded_images + 1024 * 1024);
}

/// Codex's OAuth wire must cross the same signed proxy as key providers: no live credentials.
#[tokio::test]
async fn codex_oauth_responses_publish_context_usage_and_deliver_mid_turn_steering() {
    type Seen = Arc<Mutex<Vec<(HeaderMap, Value)>>>;
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    async fn codex(AxumState(seen): AxumState<Seen>, headers: HeaderMap, body: Bytes) -> Response {
        // pi sends its own content-type; a second one makes upstream answer
        // `{"detail":"Unsupported content type"}`.
        assert_eq!(headers.get_all(header::CONTENT_TYPE).iter().count(), 1);
        assert_eq!(headers[header::CONTENT_TYPE], "application/json");
        assert_eq!(headers[header::CONTENT_ENCODING], "zstd");
        let decoded = zstd::stream::decode_all(body.as_ref()).unwrap();
        let body: Value = serde_json::from_slice(&decoded).unwrap();
        seen.lock().unwrap().push((headers, body));
        Response::builder().header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from("event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_codex\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":42000,\"output_tokens\":10,\"input_tokens_details\":{\"cached_tokens\":40000}}}}\n\n"))
            .unwrap()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = AxumRouter::new()
        .route("/backend-api/codex/responses", post(codex))
        .with_state(seen.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |config| {
        config.with_provider_origin(format!("http://{addr}"))
    })
    .await;
    let mut events = fixture.events.subscribe();
    fixture
        .steers
        .queue("ses_1")
        .enqueue(
            emberloop_core::steer::SteerKind::Steer,
            "steer: stop editing and explain",
        )
        .await;
    let body = json!({"model":"gpt-6-luna", "stream":true,"store":false, "instructions":"Original instructions", "input":[
        {"role":"user","content":[{"type":"input_text","text":"inspect this"}]},
        {"type":"function_call","id":"fc_item_1","call_id":"call_read","name":"read","arguments":"{}"},
        {"type":"function_call_output","call_id":"call_read","output":"source code"}
    ]});
    let response = router(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/proxy/{}/x/openai-codex/backend-api/codex/responses",
                    fixture.token
                ))
                .header(header::AUTHORIZATION, "Bearer fake-oauth-access")
                .header("chatgpt-account-id", "fake-account")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CONTENT_ENCODING, "zstd")
                .body(Body::from(
                    zstd::stream::encode_all(body.to_string().as_bytes(), 1).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("response.completed"));
    let captured = seen.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].0[header::AUTHORIZATION],
        "Bearer fake-oauth-access"
    );
    assert_eq!(captured[0].0["chatgpt-account-id"], "fake-account");
    assert!(captured[0].1["input"]
        .to_string()
        .contains("steer: stop editing and explain"));
    assert!(captured[0].1["instructions"]
        .as_str()
        .unwrap()
        .contains("Original instructions"));
    assert!(captured[0].1["instructions"]
        .as_str()
        .unwrap()
        .contains("Emberloop proxy policy"));
    let mut used = false;
    let mut delivered = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == EventKind::Usage {
            assert_eq!(event.data["inputTokens"], 42000);
            assert_eq!(event.data["cachedTokens"], 40000);
            assert!(event.data["breakdown"]["system"].as_u64().unwrap() > 0);
            used = true;
        }
        if event.kind == EventKind::SteerDelivered {
            delivered = true;
        }
    }
    assert!(
        used && delivered,
        "Codex must report both usage and steer delivery"
    );
}

#[tokio::test]
async fn codex_zstd_compression_is_executed_in_proxy_and_followed_up_with_folded_context() {
    type Seen = Arc<Mutex<Vec<Value>>>;
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    async fn upstream(
        AxumState(seen): AxumState<Seen>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        assert_eq!(headers[header::CONTENT_ENCODING], "zstd");
        let body: Value =
            serde_json::from_slice(&zstd::stream::decode_all(body.as_ref()).unwrap()).unwrap();
        let round = {
            let mut seen = seen.lock().unwrap();
            let n = seen.len();
            seen.push(body);
            n
        };
        let events = if round == 0 {
            let args = r#"{"start":0,"end":2,"summary":"folded codex context"}"#;
            vec![
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_fold","call_id":"call_fold","name":"compress","arguments":""}}),
                json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":args}),
                json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_fold","call_id":"call_fold","name":"compress","arguments":args}}),
                json!({"type":"response.completed","response":{"id":"resp_fold","status":"completed","output":[],"usage":{"input_tokens":42000,"output_tokens":100}}}),
            ]
        } else {
            vec![
                json!({"type":"response.output_text.delta","delta":"done after codex compression"}),
                json!({"type":"response.completed","response":{"id":"resp_final","status":"completed","output":[],"usage":{"input_tokens":9000,"output_tokens":10}}}),
            ]
        };
        let stream = events
            .iter()
            .map(|v| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap()))
            .collect::<String>();
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(stream))
            .unwrap()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = AxumRouter::new()
        .route("/backend-api/codex/responses", post(upstream))
        .with_state(seen.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let fixture = fixture_config(FakeUpstream::new(), |c| {
        c.with_provider_origin(format!("http://{addr}"))
    })
    .await;
    fixture.settings.write().await.preserve_recent_messages = 0;
    let mut events = fixture.events.subscribe();
    let request = json!({"model":"gpt-6.1-sol","stream":true,"input":[
        {"role":"user","content":"old question"}, {"role":"assistant","content":"old answer"}, {"role":"user","content":"continue"}
    ]});
    let response = router(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/proxy/{}/x/openai-codex/backend-api/codex/responses",
                    fixture.token
                ))
                .header(header::CONTENT_ENCODING, "zstd")
                .body(Body::from(
                    zstd::stream::encode_all(request.to_string().as_bytes(), 1).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("done after codex compression"), "{text}");
    assert!(
        !text.contains("call_fold"),
        "proxy tool calls must not reach the harness"
    );
    let captured = seen.lock().unwrap();
    assert_eq!(captured.len(), 2);
    let folded = captured[1]["input"].to_string();
    assert!(folded.contains("folded codex context"));
    assert!(!folded.contains("old question"));
    let mut final_input = 0;
    let mut compressed = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == EventKind::Compression {
            compressed = true;
        }
        if event.kind == EventKind::Usage {
            final_input = event.data["inputTokens"].as_u64().unwrap();
        }
    }
    assert!(compressed);
    assert_eq!(final_input, 9000);
}

/// Prefix stability: a prompt cache hits only when the request prefix is byte-identical to the
/// previous request. The harness resends its whole history, so each request must begin with the
/// previous one. Allowed break points, declared by each scenario and never inferred from a diff:
///
/// | break            | caused by                                  | prefix that must still match |
/// |------------------|--------------------------------------------|------------------------------|
/// | `Fold(start)`    | a `compress` or preflight fold from `start` | turns `..start`             |
/// | `Reset`          | `reset_history`                            | nothing                      |
///
/// Everything else must hold: request N+1 begins with every turn of request N except the
/// transient `_nudge` turn (the wire never carries its marker, so it is found by its text) and
/// the newest turn of request N, which the harness is still extending.
#[derive(Clone, Copy)]
enum Break {
    Fold(usize),
    Reset,
}

/// Each step's tool result: large enough to grow the context quickly, small enough (under the
/// 4000-token ctx threshold) that it is never replaced by a handle.
const STEP_RESULT_CHARS: usize = 12_000;

fn is_nudge_turn(turn: &Value) -> bool {
    turn["parts"][0]["text"]
        .as_str()
        .is_some_and(|text| text.starts_with("This is an efficiency nudge"))
}

/// The turns request N must still open with for request N+1: nudge and newest turn removed.
fn stable_turns(body: &Value) -> Vec<Value> {
    let mut turns: Vec<Value> = body["contents"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|turn| !is_nudge_turn(turn))
        .collect();
    turns.pop();
    turns
}

fn assert_prefix_stable(label: &str, previous: &Value, next: &Value, brk: Option<Break>) {
    let before = stable_turns(previous);
    let after = next["contents"].as_array().expect("contents");
    let must_match = match brk {
        None => before.len(),
        Some(Break::Fold(start)) => start.min(before.len()),
        Some(Break::Reset) => 0,
    };
    assert!(
        after.len() >= must_match,
        "{label}: the next request has {} turns, the previous one needs {must_match} kept",
        after.len()
    );
    for (index, turn) in before[..must_match].iter().enumerate() {
        let same = after[index] == *turn;
        assert!(
            same,
            "{label}: turn {index} of the previous request changed on the next one \
             (previous {} bytes, next {} bytes); the prompt cache misses from here",
            turn.to_string().len(),
            after[index].to_string().len()
        );
    }
}

/// Appends one model call and its user-role result, the way a harness grows its history.
fn grow_step(history: &mut Vec<Value>, step: usize) {
    let id = format!("call_{step}");
    history.push(json!({
        "role": "model",
        "parts": [{ "functionCall": { "name": "bash", "id": id, "args": { "cmd": format!("step {step}") } } }]
    }));
    history.push(json!({
        "role": "user",
        "parts": [{ "functionResponse": {
            "name": "bash",
            "id": id,
            "response": { "result": format!("{step}:{}", "x".repeat(STEP_RESULT_CHARS)) }
        } }]
    }));
}

/// Drives `steps` requests on one Session. `before` runs first for each request, may change
/// the Session's settings or script the upstream, and returns the break it declares before that
/// request. Each request is answered "done", which the harness appends to its history.
/// Returns, per request, its first upstream body (the one the cache sees before any fold the
/// request itself performs) and the declared break.
async fn drive_growing(
    fixture: &Fixture,
    warm: usize,
    steps: usize,
    mut before: impl FnMut(usize, &Fixture) -> Option<Break>,
) -> Vec<(Value, Option<Break>)> {
    // `warm` steps are already in the history before the first request, as after a long run.
    let mut history = warm_history(warm);
    let mut requests = Vec::new();
    for index in 0..steps {
        let brk = before(index, fixture);
        if matches!(brk, Some(Break::Reset)) {
            fixture.state.reset_history("ses_1").await.unwrap();
            history = warm_history(0);
        }
        grow_step(&mut history, warm + index);
        let first_body = fixture.fake.bodies.lock().unwrap().len();
        let (status, _) = send(
            &fixture.state,
            &fixture.token,
            "v1internal:generateContent",
            body(Value::Array(history.clone())),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "request {index}");
        requests.push((fixture.fake.body(first_body), brk));
        history.push(json!({ "role": "model", "parts": [{ "text": "done" }] }));
    }
    requests
}

fn assert_run_is_prefix_stable(label: &str, requests: &[(Value, Option<Break>)]) {
    for (index, pair) in requests.windows(2).enumerate() {
        let next_break = pair[1].1;
        assert_prefix_stable(
            &format!("{label}, request {}", index + 1),
            &pair[0].0,
            &pair[1].0,
            next_break,
        );
    }
}

fn echoed_usage_fake() -> FakeUpstream {
    let fake = text_only_fake();
    fake.echo_usage.store(true, Ordering::SeqCst);
    fake
}

#[tokio::test]
async fn prefix_is_stable_while_the_conversation_grows() {
    let fixture = fixture_with(echoed_usage_fake()).await;
    let requests = drive_growing(&fixture, 0, 6, |_, _| None).await;
    assert_run_is_prefix_stable("steady growth", &requests);
    let nudges = requests
        .iter()
        .filter(|(body, _)| nudge_parts(body).len() == 1)
        .count();
    assert_eq!(nudges, 0, "below the first threshold no nudge is sent");
}

#[tokio::test]
async fn prefix_is_stable_across_the_first_nudge() {
    let fixture = fixture_with(echoed_usage_fake()).await;
    // Twenty-two steps are warm (about 66000 tokens); the first nudge fires within the run, at
    // about 75000 tokens. Fewer driven requests keep the debug-build run short.
    let requests = drive_growing(&fixture, 22, 6, |_, _| None).await;
    let fired: Vec<usize> = requests
        .iter()
        .enumerate()
        .filter(|(_, (body, _))| !nudge_parts(body).is_empty())
        .map(|(index, _)| index)
        .collect();
    assert_eq!(fired.len(), 1, "the first nudge fires once: {fired:?}");
    assert_run_is_prefix_stable("first nudge", &requests);
}

#[tokio::test]
async fn prefix_is_stable_across_a_mid_run_compress() {
    let fixture = fixture_with(echoed_usage_fake()).await;
    let folded_at = 5;
    let requests = drive_growing(&fixture, 0, 8, |index, fixture| {
        if index == folded_at {
            // The model folds turns 1..4 (the first call and its result) on this request.
            fixture.fake.script.lock().unwrap().extend([
                json!({"response": {"candidates": [{"content": {"role": "model", "parts": [{
                    "functionCall": {"id": "call_c", "name": "compress",
                        "args": {"start": 1, "end": 4, "summary": "folded early steps"}},
                    "thoughtSignature": "SIG-C"
                }]}}]}}),
                json!({"response": {"candidates": [{"content": {"role": "model", "parts": [{ "text": "done" }]}}]}}),
            ]);
        }
        (index == folded_at + 1).then_some(Break::Fold(1))
    })
    .await;
    let after_fold = &requests[folded_at + 1].0["contents"];
    assert!(
        after_fold[1]["parts"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("folded early steps")),
        "the fold replaced turns 1.. on the next request"
    );
    assert_run_is_prefix_stable("mid-run compress", &requests);
}

/// A proxy-only pair whose anchor turn is then folded away: the pair is re-placed once, stays
/// there, and the Session keeps its prefix after the fold.
#[tokio::test]
async fn prefix_is_stable_after_a_compress_folds_the_pairs_anchor() {
    let fixture = fixture_with(echoed_usage_fake()).await;
    let pair_id = "call_s";
    let requests = drive_growing(&fixture, 0, 9, |index, fixture| {
        if index == 1 {
            // `acp_status` is answered by the proxy, so the pair follows this request's newest result.
            fixture.fake.script.lock().unwrap().extend([
                json!({"response": {"candidates": [{"content": {"role": "model", "parts": [{
                    "functionCall": {"id": pair_id, "name": "acp_status", "args": {}},
                    "thoughtSignature": "SIG-S"
                }]}}]}}),
                json!({"response": {"candidates": [{"content": {"role": "model", "parts": [{ "text": "done" }]}}]}}),
            ]);
        }
        if index == 5 {
            // Turns 1..8 hold the anchor result and the pair (turn 0 is the task).
            fixture.fake.script.lock().unwrap().extend([
                json!({"response": {"candidates": [{"content": {"role": "model", "parts": [{
                    "functionCall": {"id": "call_c", "name": "compress",
                        "args": {"start": 1, "end": 8, "summary": "folded the anchor"}},
                    "thoughtSignature": "SIG-C"
                }]}}]}}),
                json!({"response": {"candidates": [{"content": {"role": "model", "parts": [{ "text": "done" }]}}]}}),
            ]);
        }
        (index == 6).then_some(Break::Fold(1))
    })
    .await;
    let pair_responses = |body: &Value| {
        body["contents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|turn| {
                turn["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|part| part["functionResponse"]["id"].as_str() == Some(pair_id))
            })
            .count()
    };
    assert!(
        requests[6].0["contents"][1]["parts"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("folded the anchor")),
        "the fold replaced the anchor turn on the next request"
    );
    for (index, (body, _)) in requests.iter().enumerate().skip(2) {
        let count = pair_responses(body);
        // The pair exists from request 2 on, once: a fold that drops it is re-placed, never doubled.
        assert_eq!(count, 1, "request {index} carries the pair once");
    }
    assert_run_is_prefix_stable("folded anchor", &requests);
}

#[tokio::test]
async fn prefix_is_stable_after_emergency_truncation_stops() {
    let fixture = fixture_with(echoed_usage_fake()).await;
    // Four steps are already in the history (about 12000 tokens), so the first request is in
    // emergency at a 1 percent band: older results are truncated from the start and no request
    // sees the untruncated history.
    fixture
        .settings
        .try_write()
        .unwrap()
        .emergency_threshold_percent = 1;
    let emergency_requests = 8;
    let requests = drive_growing(&fixture, 4, 12, |index, fixture| {
        if index == emergency_requests {
            // The band no longer covers the request: the context is under the emergency line.
            fixture
                .settings
                .try_write()
                .unwrap()
                .emergency_threshold_percent = 95;
        }
        None
    })
    .await;
    for (index, (body, _)) in requests[..emergency_requests].iter().enumerate() {
        assert!(
            newest_result(body).contains("emergency truncation"),
            "emergency request {index} truncates the result that arrived with it"
        );
    }
    assert_run_is_prefix_stable("emergency", &requests);
}

#[tokio::test]
async fn prefix_breaks_at_a_reset_restart_the_conversation() {
    let fixture = fixture_with(echoed_usage_fake()).await;
    let requests = drive_growing(&fixture, 0, 6, |index, _| {
        (index == 3).then_some(Break::Reset)
    })
    .await;
    assert!(
        requests[3].0["contents"].as_array().unwrap().len()
            < requests[2].0["contents"].as_array().unwrap().len(),
        "the reset restarted the harness conversation"
    );
    assert_run_is_prefix_stable("reset", &requests);
}

fn newest_result(body: &Value) -> String {
    body["contents"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find_map(|turn| {
            turn["parts"]
                .as_array()?
                .iter()
                .find_map(|part| part["functionResponse"]["response"]["result"].as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// `steps` finished calls and answers, as a harness history that has run for a while.
fn warm_history(steps: usize) -> Vec<Value> {
    let mut history = vec![json!({ "role": "user", "parts": [{ "text": "start the task" }] })];
    for step in 0..steps {
        grow_step(&mut history, step);
        history.push(json!({ "role": "model", "parts": [{ "text": "done" }] }));
    }
    history
}

#[tokio::test]
async fn a_compress_clears_the_sticky_emergency_flag() {
    let fixture = fixture().await;
    let runtime = fixture.state.runtime("ses_1").await.unwrap();
    runtime.emergency_truncated.store(true, Ordering::Relaxed);
    let mut history = warm_history(4);
    let settings = api::state::Settings {
        preserve_recent_messages: 0,
        ..api::state::Settings::default()
    };
    let call = ProxyCall {
        name: "compress".into(),
        id: "call_c".into(),
        part: json!({}),
        args: json!({"start": 1, "end": 3, "summary": "folded"}),
    };
    execute_tool(&runtime, &mut history, &call, &settings, 1_000_000).unwrap();
    assert!(!runtime.emergency_truncated.load(Ordering::Relaxed));
}

#[tokio::test]
async fn a_preflight_fold_clears_the_sticky_emergency_flag() {
    let fixture = fixture().await;
    let runtime = fixture.state.runtime("ses_1").await.unwrap();
    runtime.emergency_truncated.store(true, Ordering::Relaxed);
    // Twenty steps are about 60000 tokens: over this window, so the preflight folds them.
    let mut history = warm_history(20);
    history.push(json!({ "role": "user", "parts": [{ "text": "continue" }] }));
    let mut request = body(Value::Array(history));
    let settings = api::state::Settings {
        preserve_recent_messages: 0,
        ..api::state::Settings::default()
    };
    let outcome = prepare_request(
        &runtime,
        WireKind::Google,
        &mut request,
        50_000,
        &settings,
        false,
    )
    .unwrap();
    assert!(outcome.preflight.is_some(), "the preflight fold ran");
    assert!(!runtime.emergency_truncated.load(Ordering::Relaxed));
}

/// The text of the result a harness call id answered, as a request body carries it.
fn result_text(body: &Value, call_id: &str) -> Option<String> {
    body["contents"]
        .as_array()?
        .iter()
        .flat_map(|turn| turn["parts"].as_array().cloned().unwrap_or_default())
        .find(|part| part["functionResponse"]["id"].as_str() == Some(call_id))
        .and_then(|part| {
            part["functionResponse"]["response"]["result"]
                .as_str()
                .map(str::to_string)
        })
}

/// Emergency truncation shortens what the model is sent, never what a compress keeps: a compress
/// run in an emergency request stores the full results, so decompress restores them byte for byte.
#[tokio::test]
async fn a_compress_during_an_emergency_restores_the_full_results() {
    let fixture = fixture_with(echoed_usage_fake()).await;
    {
        let mut settings = fixture.settings.try_write().unwrap();
        settings.emergency_threshold_percent = 1;
        settings.preserve_recent_messages = 0;
    }
    let mut history = warm_history(4);
    history.push(json!({ "role": "user", "parts": [{ "text": "continue" }] }));
    let end = history.len() - 1;
    fixture.fake.script.lock().unwrap().extend([
        json!({"response": {"candidates": [{"content": {"role": "model", "parts": [{
            "functionCall": {"id": "call_c", "name": "compress",
                "args": {"start": 1, "end": end, "summary": "old steps"}},
            "thoughtSignature": "SIG-C"
        }]}}]}}),
        json!({"response": {"candidates": [{"content": {"role": "model", "parts": [{
            "functionCall": {"id": "call_d", "name": "decompress", "args": {"blockId": "b1"}},
            "thoughtSignature": "SIG-D"
        }]}}]}}),
        json!({"response": {"candidates": [{"content": {"role": "model", "parts": [{ "text": "done" }]}}]}}),
    ]);
    let (status, _) = send(
        &fixture.state,
        &fixture.token,
        "v1internal:generateContent",
        body(Value::Array(history)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let full = format!("0:{}", "x".repeat(STEP_RESULT_CHARS));
    assert!(
        result_text(&fixture.fake.body(0), "call_0")
            .is_some_and(|text| text.contains("emergency truncation")),
        "the emergency request sent the first result truncated"
    );
    assert_eq!(
        result_text(&fixture.fake.body(2), "call_0").as_deref(),
        Some(full.as_str()),
        "decompress restores the result as the harness sent it"
    );
}
