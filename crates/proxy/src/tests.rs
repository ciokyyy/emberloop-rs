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
}

impl FakeUpstream {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            bodies: Arc::new(Mutex::new(Vec::new())),
            sse: Arc::new(Mutex::new(VecDeque::new())),
            hang: Arc::new(Mutex::new(None)),
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
    fake.calls.fetch_add(1, Ordering::SeqCst);
    fake.bodies.lock().unwrap().push(body);
    if let Some(response) = fake.sse.lock().unwrap().pop_front() {
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(response))
            .unwrap();
    }
    let call = fake.calls.load(Ordering::SeqCst) - 1;
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
                "promptTokenCount": 11,
                "candidatesTokenCount": 7,
                "totalTokenCount": 18
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
    events: Arc<EventBus>,
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
        .with_state(fake.clone());
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let events = Arc::new(EventBus::new(16));
    let prompts = Arc::new(PromptStore::new(dir.path().join("prompts")));
    let steers = emberloop_core::steer::SteerQueues::default();
    let state = ProxyState::new(
        configure(ProxyConfig::new(b"test-secret").with_upstream(format!("http://{addr}"))),
        sessions.clone(),
        projects,
        data_root,
        prompts,
        events.clone(),
        steers.clone(),
    );
    let token = state.token_for("ses_1");

    Fixture {
        state,
        steers,
        sessions,
        events,
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
                .uri(format!("/p/{token}/{path}"))
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
    assert!(system.contains("demo"));

    let comp_event = events.try_recv().expect("compression event");
    assert_eq!(comp_event.kind, EventKind::Compression);
    let event = events.try_recv().expect("usage event");
    assert_eq!(event.kind, EventKind::Usage);
    assert_eq!(event.data["inputTokens"], 11);
    assert_eq!(event.data["outputTokens"], 7);

    let meta = fixture.sessions.get("ses_1").await.unwrap();
    assert!(!meta.prompt_hashes.is_empty());
    assert!(meta.prompt_hashes.contains_key(prompts::POLICY));
    assert!(meta.prompt_hashes.contains_key(prompts::PONYTAIL));
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
            "/p/{}/v1internal:streamGenerateContent",
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
    let fixture = fixture_with(FakeUpstream::hanging(dropped)).await;
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

fn pi_upstream(seen: Seen) -> AxumRouter {
    async fn chat(
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
    AxumRouter::new()
        .route("/inference/go/openai/v1/chat/completions", post(chat))
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
                .uri(format!("/p/{}/v1/chat/completions", fixture.token))
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
                .uri(format!("/p/{}/v1/v1/messages", fixture.token))
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
                .uri(format!("/p/{}/v1/responses", fixture.token))
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
