use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, Response, StatusCode, Uri};
use axum::routing::{any, get};
use axum::Router;
use emberloop_core::events::{EventBus, EventKind, SessionEvent};
use emberloop_core::steer::{delivered_event, SteerEntry, SteerQueue, SteerQueues};
use futures::{Stream, StreamExt};
use prompts::{PinnedPrompts, PromptError, PromptStore, PromptVars};
use serde_json::{json, Value};
use store::{DataRoot, ProjectsStore, SessionsStore};
use tokio::time::timeout;

use crate::compressor::{CompressCall, Compressor, DecompressCall, NativeCompressor, SearchHit};
use crate::meter;
use crate::splice::{Splice, SpliceStore};
use crate::token::{TokenError, TokenSigner};
use crate::wire::{
    extract_core_contents, extract_proxy_call_from_payload, inject_prompts_for_wire,
    inject_tools_for_wire, write_core_contents, WireKind,
};
pub const DEFAULT_UPSTREAM: &str = "https://daily-cloudcode-pa.googleapis.com";
/// Unverified with a real key: docs.opencode.ai names `/inference/go`, third-party guides `/zen/go/v1`.
pub const DEFAULT_OPENCODE_UPSTREAM: &str = "https://opencode.ai/inference/go";
#[cfg(not(test))]
const ROUND_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(test)]
const ROUND_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_TOOL_ROUNDS: usize = 8;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const PROXY_TOOLS: &[&str] = &["compress", "decompress", "search_context"];

type ByteStream = std::pin::Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// Absolute upstream origin, e.g. the Cloud Code CCPA endpoint.
    pub upstream_base: String,
    /// HMAC key for Session tokens. The daemon generates one per run.
    pub token_secret: Vec<u8>,
    /// OpenCode Go origin the pi wires (chat, responses, messages) stream to.
    pub opencode_upstream: String,
}

impl ProxyConfig {
    pub fn new(token_secret: impl Into<Vec<u8>>) -> Self {
        Self {
            upstream_base: DEFAULT_UPSTREAM.to_string(),
            token_secret: token_secret.into(),
            opencode_upstream: DEFAULT_OPENCODE_UPSTREAM.to_string(),
        }
    }

    pub fn with_upstream(mut self, upstream_base: impl Into<String>) -> Self {
        self.upstream_base = upstream_base.into();
        self
    }
}

impl ProxyConfig {
    pub fn with_opencode_upstream(mut self, upstream_base: impl Into<String>) -> Self {
        self.opencode_upstream = upstream_base.into();
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("invalid session token")]
    Token(#[from] TokenError),
    #[error("unknown session")]
    UnknownSession,
    #[error("prompt error: {0}")]
    Prompt(#[from] PromptError),
    #[error("invalid request body: {0}")]
    Json(#[from] serde_json::Error),
    #[error("upstream error: {0}")]
    Upstream(#[from] reqwest::Error),
    #[error("request body too large")]
    BodyTooLarge,
    #[error("request has no conversation contents")]
    NoContents,
    #[error("proxy tool round limit reached")]
    ToolRoundLimit,
    #[error("upstream round timed out")]
    Timeout,
    #[error("splice store: {0}")]
    Splice(#[from] std::io::Error),
    #[error("lock poisoned")]
    Lock,
}

#[derive(Clone)]
pub struct ProxyState {
    inner: Arc<ProxyInner>,
}

struct ProxyInner {
    config: ProxyConfig,
    signer: TokenSigner,
    sessions: SessionsStore,
    projects: ProjectsStore,
    data_root: DataRoot,
    prompts: Arc<PromptStore>,
    events: Arc<EventBus>,
    steers: SteerQueues,
    client: reqwest::Client,
    runtimes: RwLock<HashMap<String, Arc<SessionRuntime>>>,
}

struct SessionRuntime {
    pins: PinnedPrompts,
    splices: Mutex<SpliceStore>,
    compressor: Mutex<Box<dyn Compressor>>,
    last_input_tokens: AtomicU64,
}

impl SessionRuntime {
    fn new(
        pins: PinnedPrompts,
        spill_dir: std::path::PathBuf,
        splices_file: std::path::PathBuf,
    ) -> Self {
        Self {
            pins,
            splices: Mutex::new(SpliceStore::open(splices_file)),
            compressor: Mutex::new(Box::new(NativeCompressor::new(spill_dir))),
            last_input_tokens: AtomicU64::new(0),
        }
    }
}

impl ProxyState {
    pub fn new(
        config: ProxyConfig,
        sessions: SessionsStore,
        projects: ProjectsStore,
        data_root: DataRoot,
        prompts: Arc<PromptStore>,
        events: Arc<EventBus>,
        steers: SteerQueues,
    ) -> Self {
        let signer = TokenSigner::new(config.token_secret.clone());
        Self {
            inner: Arc::new(ProxyInner {
                config,
                signer,
                sessions,
                projects,
                data_root,
                prompts,
                events,
                steers,
                client: reqwest::Client::new(),
                runtimes: RwLock::new(HashMap::new()),
            }),
        }
    }

    pub fn token_for(&self, session_id: &str) -> String {
        self.inner.signer.sign(session_id)
    }

    /// The Session's harness conversation started over, so the splices and
    /// compressed blocks stored for the old one no longer fit its history.
    pub async fn reset_history(&self, session_id: &str) -> Result<(), ProxyError> {
        let runtime = self.runtime(session_id).await?;
        lock(&runtime.splices)?.reset()?;
        let spill = self.inner.data_root.session_spill_dir(session_id);
        *lock(&runtime.compressor)? = Box::new(NativeCompressor::new(spill));
        Ok(())
    }

    async fn runtime(&self, session_id: &str) -> Result<Arc<SessionRuntime>, ProxyError> {
        if let Some(runtime) = self.cached_runtime(session_id)? {
            return Ok(runtime);
        }

        let meta = self
            .inner
            .sessions
            .get(session_id)
            .await
            .ok_or(ProxyError::UnknownSession)?;
        let project = self.inner.projects.get(&meta.project_id).await;
        let vars = PromptVars {
            project_name: project.as_ref().map(|p| p.name.clone()).unwrap_or_default(),
            project_root: project
                .as_ref()
                .map(|p| p.root.to_string_lossy().to_string())
                .unwrap_or_default(),
            level: meta.ponytail_level.clone(),
            date: String::new(),
            session_id: session_id.to_string(),
        };
        let pins = PinnedPrompts::pin(&self.inner.prompts, &vars)?;
        let hashes = pins.hashes();
        if meta.prompt_hashes != hashes {
            // A resumed Session cannot recover text pinned by a previous
            // process, so it records the text it pins now.
            let _ = self
                .inner
                .sessions
                .set_prompt_hashes(session_id, hashes)
                .await;
        }
        let spill = self.inner.data_root.session_spill_dir(session_id);
        let splices = self.inner.data_root.session_splices_file(session_id);
        let runtime = Arc::new(SessionRuntime::new(pins, spill, splices));

        let mut runtimes = self.inner.runtimes.write().map_err(|_| ProxyError::Lock)?;
        Ok(runtimes
            .entry(session_id.to_string())
            .or_insert(runtime)
            .clone())
    }

    fn cached_runtime(&self, session_id: &str) -> Result<Option<Arc<SessionRuntime>>, ProxyError> {
        Ok(self
            .inner
            .runtimes
            .read()
            .map_err(|_| ProxyError::Lock)?
            .get(session_id)
            .cloned())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ProxyError> {
    mutex.lock().map_err(|_| ProxyError::Lock)
}

pub fn router(state: ProxyState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/p/{token}/{*rest}", any(handler))
        .with_state(state)
}

async fn handler(
    State(state): State<ProxyState>,
    Path((token, rest)): Path<(String, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    match dispatch(state, token, rest, method, uri, headers, body).await {
        Ok(response) => response,
        Err(error) => error_response(&error),
    }
}

async fn dispatch(
    state: ProxyState,
    token: String,
    rest: String,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Result<Response<Body>, ProxyError> {
    let session_id = state.inner.signer.verify(&token)?;
    let body = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|_| ProxyError::BodyTooLarge)?;

    let Some((wire, pi_path)) = WireKind::from_rest(&rest) else {
        return forward_plain(&state, &method, &rest, uri.query(), &headers, body).await;
    };
    handle_generation(
        &state,
        &session_id,
        wire,
        pi_path.as_deref(),
        &rest,
        uri.query(),
        &headers,
        body,
    )
    .await
}

/// The OpenCode Go path for a pi wire, by the request path suffix pi appends
/// to its base URL; `None` for everything else (Cloud Code, plain forwards).
#[cfg(test)]
fn pi_upstream_path(rest: &str) -> Option<String> {
    if rest.ends_with("/chat/completions") {
        Some("openai/v1/chat/completions".to_string())
    } else if rest.ends_with("/responses") {
        Some("openai/v1/responses".to_string())
    } else if rest.ends_with("/messages") {
        Some("anthropic/v1/messages".to_string())
    } else {
        rest.strip_prefix("v1/models/")
            .map(|model| format!("google/v1beta/models/{model}"))
    }
}

async fn forward_plain(
    state: &ProxyState,
    method: &Method,
    rest: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response<Body>, ProxyError> {
    let url = upstream_url(&state.inner.config.upstream_base, rest, query);
    let mut request = state.inner.client.request(method.clone(), url);
    for (name, value) in headers {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        request = request.header(name.as_str(), value.as_bytes());
    }
    let response = request.body(body).send().await?;
    let status = axum_status(response.status());
    let content_type = header_value(response.headers(), header::CONTENT_TYPE.as_str());
    let bytes = response.bytes().await?;
    Ok(response_with(status, content_type.as_deref(), bytes))
}

#[allow(clippy::too_many_arguments)]
async fn handle_generation(
    state: &ProxyState,
    session_id: &str,
    wire: WireKind,
    pi_path: Option<&str>,
    rest: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response<Body>, ProxyError> {
    let runtime = state.runtime(session_id).await?;
    let mut request: Value = serde_json::from_slice(&body)?;
    let queue = state.inner.steers.queue(session_id);
    queue.begin_request().await;
    let injected = inject_steer(&runtime, wire, &mut request, &queue).await;
    prepare_request(&runtime, wire, &mut request)?;

    let url = match pi_path {
        Some(path) => upstream_url(&state.inner.config.opencode_upstream, path, query),
        None => upstream_url(&state.inner.config.upstream_base, rest, query),
    };
    let response = match send_stream(&state.inner.client, &url, headers, &request).await {
        Ok(response) => response,
        Err(error) => {
            settle_steer(&queue, &injected, None, &state.inner.events, session_id).await;
            return Err(error);
        }
    };
    let status = axum_status(response.status());
    settle_steer(
        &queue,
        &injected,
        Some(status.as_u16()),
        &state.inner.events,
        session_id,
    )
    .await;
    let content_type = header_value(response.headers(), header::CONTENT_TYPE.as_str());
    if !status.is_success() {
        let bytes = response.bytes().await?;
        return Ok(response_with(status, content_type.as_deref(), bytes));
    }
    if content_type
        .as_deref()
        .is_some_and(|value| value.contains("text/event-stream"))
    {
        return Ok(streaming_response(SseState::new(
            state.clone(),
            runtime,
            session_id.to_string(),
            wire,
            url,
            headers.clone(),
            request,
            response,
        )));
    }
    let first_bytes = response.bytes().await?;
    buffered_generation(
        state,
        runtime,
        session_id,
        wire,
        url,
        headers,
        request,
        (status, content_type, first_bytes),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn buffered_generation(
    state: &ProxyState,
    runtime: Arc<SessionRuntime>,
    session_id: &str,
    wire: WireKind,
    url: String,
    headers: &HeaderMap,
    mut request: Value,
    first: (StatusCode, Option<String>, Bytes),
) -> Result<Response<Body>, ProxyError> {
    let mut final_response = None;
    let mut current = Some(first);
    for _ in 0..MAX_TOOL_ROUNDS {
        let (status, content_type, bytes) = match current.take() {
            Some(response) => response,
            None => send_generation(&state.inner.client, &url, headers, &request).await?,
        };
        if !status.is_success() {
            return Ok(response_with(status, content_type.as_deref(), bytes));
        }
        let Some(call) = extract_proxy_call(&bytes) else {
            final_response = Some((status, content_type, bytes));
            break;
        };
        let turns = run_proxy_tool(state, session_id, &runtime, wire, &mut request, call)?;
        store_proxy_splice(&runtime, &turns)?;
    }

    let Some((status, content_type, bytes)) = final_response else {
        return Err(ProxyError::ToolRoundLimit);
    };
    publish_usage(&state.inner.events, session_id, &runtime, &bytes);
    Ok(response_with(status, content_type.as_deref(), bytes))
}

fn prepare_request(
    runtime: &SessionRuntime,
    wire: WireKind,
    request: &mut Value,
) -> Result<(), ProxyError> {
    let request = request_body_mut(request);

    inject_tools_for_wire(wire, request, &runtime.pins);
    inject_prompts_for_wire(wire, request, &runtime.pins);
    let Some(mut contents) = extract_core_contents(wire, request) else {
        return Ok(());
    };
    lock(&runtime.compressor)?.apply(&mut contents);
    lock(&runtime.splices)?.apply(&mut contents);
    if wire == WireKind::Google {
        crate::splice::sign_unsigned_calls(&mut contents);
    }
    write_core_contents(wire, request, contents);
    Ok(())
}

/// Route 1: put every pending steer into the user turn that carries the tool
/// results, and remember the exact part so later requests replay it unchanged.
async fn inject_steer(
    runtime: &SessionRuntime,
    wire: WireKind,
    request: &mut Value,
    queue: &SteerQueue,
) -> Vec<SteerEntry> {
    let entries = queue.drain_pending().await;
    if entries.is_empty() {
        return entries;
    }
    let ids: Vec<String> = entries.iter().map(|entry| entry.id.clone()).collect();
    let Some(turn_id) = extract_core_contents(wire, request_body_mut(request))
        .and_then(|contents| last_tool_result_id(&contents))
    else {
        queue.requeue(&ids).await;
        return Vec::new();
    };
    let text = entries
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let splice = Splice {
        id: format!("steer-{}", ids.join("-")),
        tool_result_id: turn_id,
        marker: String::new(),
        turns: vec![json!({ "role": "user", "parts": [{ "text": text }] })],
        append_parts: Vec::new(),
    };
    if let Ok(mut splices) = lock(&runtime.splices) {
        let _ = splices.insert(splice);
    }
    entries
}

fn last_tool_result_id(contents: &[Value]) -> Option<String> {
    contents.iter().rev().find_map(|turn| {
        turn.get("parts")
            .and_then(Value::as_array)?
            .iter()
            .find_map(|part| {
                part.get("functionResponse")
                    .and_then(|response| response.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    })
}

/// A 2xx head means the model saw the entries; a failure before the head puts
/// them back for the next request or the idle wake.
async fn settle_steer(
    queue: &SteerQueue,
    entries: &[SteerEntry],
    status: Option<u16>,
    events: &EventBus,
    session_id: &str,
) {
    if !entries.is_empty() {
        let ids: Vec<String> = entries.iter().map(|entry| entry.id.clone()).collect();
        match status {
            Some(status) if (200..300).contains(&status) => {
                queue.mark_delivered(&ids, "injected").await;
                for entry in entries {
                    events.publish(delivered_event(session_id, entry, "injected"));
                }
            }
            _ => queue.requeue(&ids).await,
        }
    }
    queue.end_request().await;
}

fn request_body_mut(root: &mut Value) -> &mut Value {
    if root.get("request").is_some_and(Value::is_object) {
        &mut root["request"]
    } else {
        root
    }
}

#[derive(Debug, Clone)]
pub struct ProxyCall {
    pub name: String,
    pub id: String,
    pub part: Value,
    pub args: Value,
}

fn extract_proxy_call(bytes: &Bytes) -> Option<ProxyCall> {
    let text = String::from_utf8_lossy(bytes);
    for payload in meter::payloads(&text) {
        if let Some(call) = proxy_call_in(&payload) {
            return Some(call);
        }
    }
    None
}

fn proxy_call_in(payload: &Value) -> Option<ProxyCall> {
    if let Some(call) = extract_proxy_call_from_payload(payload) {
        return Some(call);
    }
    let candidates = payload
        .get("response")
        .and_then(|response| response.get("candidates"))
        .or_else(|| payload.get("candidates"))
        .and_then(Value::as_array)?;
    for candidate in candidates {
        let Some(parts) = candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for part in parts {
            if let Some(call) = proxy_call_from_part(part) {
                return Some(call);
            }
        }
    }
    None
}

fn proxy_call_from_part(part: &Value) -> Option<ProxyCall> {
    let call = part.get("functionCall")?;
    let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
    if !PROXY_TOOLS.contains(&name) {
        return None;
    }
    Some(ProxyCall {
        name: name.to_string(),
        id: call
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("call_1")
            .to_string(),
        part: part.clone(),
        args: call_arguments(call),
    })
}

fn call_arguments(call: &Value) -> Value {
    if let Some(args) = call.get("args") {
        return args.clone();
    }
    if let Some(arguments) = call.get("arguments") {
        if let Some(text) = arguments.as_str() {
            return serde_json::from_str(text).unwrap_or_else(|_| json!({}));
        }
        return arguments.clone();
    }
    json!({})
}
/// Run the tool and append its model+user pair to the request. Returns the
/// stored pair so the caller can persist it.
fn run_proxy_tool(
    state: &ProxyState,
    session_id: &str,
    runtime: &SessionRuntime,
    wire: WireKind,
    request: &mut Value,
    call: ProxyCall,
) -> Result<Vec<Value>, ProxyError> {
    let request = request_body_mut(request);
    let mut contents = extract_core_contents(wire, request).ok_or(ProxyError::NoContents)?;
    let text = execute_by_name(state, session_id, runtime, &mut contents, &call);
    let turns = proxy_pair(&call, &text, &format!("emberloop-splice:{}", call.id));
    contents.extend(turns.clone());
    write_core_contents(wire, request, contents);
    Ok(turns)
}

fn execute_by_name(
    state: &ProxyState,
    session_id: &str,
    runtime: &SessionRuntime,
    contents: &mut Vec<Value>,
    call: &ProxyCall,
) -> String {
    match execute_tool(runtime, contents, call) {
        Ok(text) => {
            if matches!(call.name.as_str(), "compress" | "decompress") {
                publish_compression_event(&state.inner.events, session_id, runtime, call, &text);
            }
            text
        }
        Err(error) => format!("error: {error}"),
    }
}

fn publish_compression_event(
    events: &EventBus,
    session_id: &str,
    runtime: &SessionRuntime,
    call: &ProxyCall,
    result_text: &str,
) {
    let status = lock(&runtime.compressor).ok().map(|c| c.status());
    events.publish(SessionEvent::new(
        session_id,
        0,
        EventKind::Compression,
        json!({
            "action": call.name,
            "args": call.args,
            "result": result_text,
            "activeBlocks": status.as_ref().map(|s| s.active_blocks).unwrap_or(0),
            "spilledTurns": status.as_ref().map(|s| s.spilled_turns).unwrap_or(0),
        }),
    ));
}

fn proxy_pair(call: &ProxyCall, text: &str, marker: &str) -> Vec<Value> {
    let response_part = json!({
        "functionResponse": {
            "name": call.name,
            "id": call.id,
            "response": { "result": text, "splice": marker }
        }
    });
    vec![
        json!({ "role": "model", "parts": [call.part.clone()] }),
        json!({ "role": "user", "parts": [response_part] }),
    ]
}

fn store_proxy_splice(runtime: &SessionRuntime, turns: &[Value]) -> Result<String, ProxyError> {
    let marker = turns
        .first()
        .and_then(|turn| turn.get("parts"))
        .and_then(Value::as_array)
        .and_then(|parts| parts.first())
        .and_then(|part| part.get("functionCall"))
        .and_then(|call| call.get("id"))
        .and_then(Value::as_str)
        .map(|id| format!("emberloop-splice:{id}"))
        .unwrap_or_else(|| "emberloop-splice:unknown".to_string());
    let anchor = turns
        .get(1)
        .and_then(|turn| turn.get("parts"))
        .and_then(Value::as_array)
        .and_then(|parts| parts.first())
        .and_then(|part| part.get("functionResponse"))
        .and_then(|response| response.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let splice = Splice::new(&anchor, turns.to_vec()).with_marker(marker);
    lock(&runtime.splices)?.insert(splice)?;
    Ok(anchor)
}

fn execute_tool(
    runtime: &SessionRuntime,
    contents: &mut Vec<Value>,
    call: &ProxyCall,
) -> Result<String, String> {
    match call.name.as_str() {
        "compress" => {
            let call = parse_compress_call(&call.args)?;
            lock(&runtime.compressor)
                .map_err(|error| error.to_string())?
                .compress(contents, call)
        }
        "decompress" => {
            let call = parse_decompress_call(&call.args)?;
            lock(&runtime.compressor)
                .map_err(|error| error.to_string())?
                .decompress(contents, call)
        }
        "search_context" => {
            let query = string_arg(&call.args, "query")?;
            let limit = call.args.get("limit").and_then(Value::as_u64).unwrap_or(5) as usize;
            let hits: Vec<SearchHit> = lock(&runtime.compressor)
                .map_err(|error| error.to_string())?
                .search_context(&query, limit)?;
            serde_json::to_string_pretty(&hits).map_err(|error| error.to_string())
        }
        other => Err(format!("not a proxy tool: {other}")),
    }
}

fn parse_compress_call(args: &Value) -> Result<CompressCall, String> {
    Ok(CompressCall {
        start: number_arg(args, "start")?,
        end: number_arg(args, "end")?,
        summary: string_arg(args, "summary")?,
        topic: args
            .get("topic")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn parse_decompress_call(args: &Value) -> Result<DecompressCall, String> {
    Ok(DecompressCall {
        block_id: string_arg(args, "blockId")?,
        to_file: args
            .get("toFile")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn string_arg(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{key} must be a string"))
}

fn number_arg(args: &Value, key: &str) -> Result<usize, String> {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .ok_or_else(|| format!("{key} must be a non-negative integer"))
}

async fn send_stream(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
) -> Result<reqwest::Response, ProxyError> {
    let bytes = serde_json::to_vec(body)?;
    let mut request = client.post(url);
    request = with_headers(request, headers);
    let request = request.header(header::CONTENT_TYPE, "application/json");
    timeout(ROUND_TIMEOUT, request.body(bytes).send())
        .await
        .map_err(|_| ProxyError::Timeout)?
        .map_err(ProxyError::Upstream)
}

async fn send_generation(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
) -> Result<(StatusCode, Option<String>, Bytes), ProxyError> {
    let response = send_stream(client, url, headers, body).await?;
    let status = axum_status(response.status());
    let content_type = header_value(response.headers(), header::CONTENT_TYPE.as_str());
    let bytes = timeout(ROUND_TIMEOUT, response.bytes())
        .await
        .map_err(|_| ProxyError::Timeout)??;
    Ok((status, content_type, bytes))
}

fn with_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        request = request.header(name.as_str(), value.as_bytes());
    }
    request
}

fn publish_usage(events: &EventBus, session_id: &str, runtime: &SessionRuntime, bytes: &Bytes) {
    if let Some(usage) = publish_usage_event(events, session_id, bytes) {
        runtime
            .last_input_tokens
            .store(usage.input_tokens, Ordering::Relaxed);
    }
}

fn publish_usage_event(events: &EventBus, session_id: &str, bytes: &Bytes) -> Option<meter::Usage> {
    let usage = meter::extract_usage(bytes)?;
    events.publish(SessionEvent::new(
        session_id,
        0,
        EventKind::Usage,
        json!({
            "inputTokens": usage.input_tokens,
            "outputTokens": usage.output_tokens
        }),
    ));
    Some(usage)
}

fn publish_error(events: &EventBus, session_id: &str, message: String) {
    events.publish(SessionEvent::new(
        session_id,
        0,
        EventKind::Error,
        json!({ "message": message }),
    ));
}

fn upstream_url(base: &str, rest: &str, query: Option<&str>) -> String {
    let mut url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        rest.trim_start_matches('/')
    );
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

fn is_hop_by_hop(name: &str) -> bool {
    ["host", "content-length", "accept-encoding"]
        .iter()
        .any(|hop| name.eq_ignore_ascii_case(hop))
}

fn axum_status(status: reqwest::StatusCode) -> StatusCode {
    StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY)
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn response_with(status: StatusCode, content_type: Option<&str>, bytes: Bytes) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder
        .body(Body::from(bytes))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn error_response(error: &ProxyError) -> Response<Body> {
    let status = match error {
        ProxyError::Token(_) | ProxyError::UnknownSession => StatusCode::UNAUTHORIZED,
        ProxyError::Json(_) | ProxyError::NoContents => StatusCode::BAD_REQUEST,
        ProxyError::BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ProxyError::Prompt(_)
        | ProxyError::ToolRoundLimit
        | ProxyError::Timeout
        | ProxyError::Splice(_)
        | ProxyError::Lock => StatusCode::BAD_GATEWAY,
        ProxyError::Upstream(_) => StatusCode::BAD_GATEWAY,
    };
    response_with(
        status,
        Some("text/plain; charset=utf-8"),
        Bytes::from(error.to_string()),
    )
}

#[derive(Debug)]
struct CompletedCall {
    call: ProxyCall,
    text: String,
}

struct SseState {
    proxy: ProxyState,
    runtime: Arc<SessionRuntime>,
    session_id: String,
    wire: WireKind,
    url: String,
    headers: HeaderMap,
    request: Value,
    stream: Option<ByteStream>,
    pending: Vec<u8>,
    /// Events decided to pass through after being buffered behind a tool call.
    forward: VecDeque<Bytes>,
    rounds: usize,
    done: bool,
    harness_call_id: Option<String>,
    completed: Vec<CompletedCall>,
    usage: Option<meter::Usage>,
    /// The signature of the step being streamed: the model signs its first call.
    step_signature: Option<Value>,
    /// Tool calls a pi wire streams in fragments; the wire decides run or hide.
    stream_calls: Vec<StreamCall>,
    stream_events: Vec<Bytes>,
    stream_finish: Vec<Bytes>,
    proxy_round: bool,
}

#[derive(Debug, Default)]
struct StreamCall {
    index: usize,
    id: String,
    name: String,
    args: String,
}

impl SseState {
    #[allow(clippy::too_many_arguments)]
    fn new(
        proxy: ProxyState,
        runtime: Arc<SessionRuntime>,
        session_id: String,
        wire: WireKind,
        url: String,
        headers: HeaderMap,
        request: Value,
        response: reqwest::Response,
    ) -> Self {
        Self {
            proxy,
            runtime,
            session_id,
            wire,
            url,
            headers,
            request,
            stream: Some(response.bytes_stream().boxed()),
            pending: Vec::new(),
            forward: VecDeque::new(),
            rounds: 1,
            done: false,
            harness_call_id: None,
            completed: Vec::new(),
            usage: None,
            step_signature: None,
            stream_calls: Vec::new(),
            stream_events: Vec::new(),
            stream_finish: Vec::new(),
            proxy_round: false,
        }
    }

    async fn follow_up(&mut self) -> Result<(), ProxyError> {
        let response = send_stream(
            &self.proxy.inner.client,
            &self.url,
            &self.headers,
            &self.request,
        )
        .await?;
        self.stream = Some(response.bytes_stream().boxed());
        self.step_signature = None;
        self.proxy_round = false;
        self.stream_calls.clear();
        self.rounds += 1;
        Ok(())
    }

    fn finish_round(&mut self) -> Result<(), ProxyError> {
        if self.completed.is_empty() {
            self.done = true;
            return Ok(());
        }
        let first_id = self.completed[0].call.id.clone();
        let marker = format!("emberloop-splice:{first_id}");
        let turns = self
            .completed
            .iter()
            .flat_map(|completed| proxy_pair(&completed.call, &completed.text, &marker).into_iter())
            .collect::<Vec<_>>();

        if let Some(anchor) = self.harness_call_id.clone() {
            let splice = Splice::new(&anchor, turns).with_marker(&marker);
            lock(&self.runtime.splices)?.insert(splice)?;
            self.done = true;
            return Ok(());
        }

        let anchor = self
            .completed
            .first()
            .map(|completed| completed.call.id.clone())
            .unwrap_or_default();
        let splice = Splice::new(&anchor, turns.clone()).with_marker(&marker);
        lock(&self.runtime.splices)?.insert(splice)?;
        let req_body = request_body_mut(&mut self.request);
        if let Some(mut contents) = extract_core_contents(self.wire, req_body) {
            contents.extend(turns);
            write_core_contents(self.wire, req_body, contents);
        }
        if self.rounds >= MAX_TOOL_ROUNDS {
            return Err(ProxyError::ToolRoundLimit);
        }
        self.completed.clear();
        Ok(())
    }

    fn process_event(&mut self, event: &[u8]) -> Result<Option<Bytes>, ProxyError> {
        let raw = delimited(event);
        let text = String::from_utf8_lossy(event);
        let Some(data) = sse_data(&text) else {
            return Ok(Some(raw));
        };
        let Ok(mut payload) = serde_json::from_str::<Value>(&data) else {
            return Ok((!self.proxy_round || data.trim() != "[DONE]").then_some(raw));
        };
        if let Some(usage) = meter::extract_usage(event) {
            self.usage = Some(usage);
        }
        if self.proxy_round && is_stream_end(&payload) {
            return Ok(None);
        }
        if self.pi_stream_event(&mut payload, raw.clone())?.is_some() {
            return Ok(None);
        }
        let outcome = rewrite_payload(&mut payload, self)?;
        if !outcome.changed {
            return Ok(Some(raw));
        }
        if outcome.empty {
            return Ok(None);
        }
        let body = serde_json::to_vec(&payload).map_err(ProxyError::Json)?;
        Ok(Some(Bytes::from(format!(
            "data: {}\n\n",
            String::from_utf8_lossy(&body)
        ))))
    }

    /// A pi wire's fragmented tool call: buffer it, then run a proxy tool and
    /// follow up, or hand the exact same events to the harness.
    fn pi_stream_event(
        &mut self,
        payload: &mut Value,
        raw: Bytes,
    ) -> Result<Option<RewriteOutcome>, ProxyError> {
        match self.wire {
            WireKind::OpenAiChat => self.openai_stream_event(payload, raw),
            WireKind::Anthropic => self.anthropic_stream_event(payload, raw),
            WireKind::OpenAiResponses => self.responses_stream_event(payload, raw),
            WireKind::Google => Ok(None),
        }
    }

    fn openai_stream_event(
        &mut self,
        payload: &mut Value,
        raw: Bytes,
    ) -> Result<Option<RewriteOutcome>, ProxyError> {
        let (saw_tool, finished) = collect_openai_calls(payload, &mut self.stream_calls);
        if !saw_tool && !(finished && !self.stream_calls.is_empty()) {
            return Ok(None);
        }
        if saw_tool && !finished {
            self.stream_events.push(raw);
        } else {
            self.stream_finish.push(raw);
        }
        if finished {
            self.decide_stream_calls()?;
        }
        Ok(Some(RewriteOutcome {
            changed: true,
            empty: true,
        }))
    }

    fn anthropic_stream_event(
        &mut self,
        payload: &mut Value,
        raw: Bytes,
    ) -> Result<Option<RewriteOutcome>, ProxyError> {
        let kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "content_block_start" => {
                if payload
                    .pointer("/content_block/type")
                    .and_then(Value::as_str)
                    != Some("tool_use")
                {
                    return Ok(None);
                }
                push_stream_call(payload, &mut self.stream_calls, false);
                self.stream_events.push(raw);
                Ok(Some(suppressed_outcome()))
            }
            "content_block_delta" => {
                if payload.pointer("/delta/type").and_then(Value::as_str)
                    != Some("input_json_delta")
                {
                    return Ok(None);
                }
                push_stream_call(payload, &mut self.stream_calls, true);
                self.stream_events.push(raw);
                Ok(Some(suppressed_outcome()))
            }
            "content_block_stop" => {
                let index = stream_index(payload);
                if !self.stream_calls.iter().any(|call| call.index == index) {
                    return Ok(None);
                }
                self.stream_events.push(raw);
                Ok(Some(suppressed_outcome()))
            }
            "message_delta" => {
                if self.stream_calls.is_empty() {
                    return Ok(None);
                }
                self.stream_finish.push(raw);
                Ok(Some(suppressed_outcome()))
            }
            "message_stop" => {
                if self.stream_calls.is_empty() {
                    return Ok(None);
                }
                self.stream_finish.push(raw);
                self.decide_stream_calls()?;
                Ok(Some(suppressed_outcome()))
            }
            _ => Ok(None),
        }
    }

    fn responses_stream_event(
        &mut self,
        payload: &mut Value,
        raw: Bytes,
    ) -> Result<Option<RewriteOutcome>, ProxyError> {
        let kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "response.output_item.added" => {
                if payload.pointer("/item/type").and_then(Value::as_str) != Some("function_call") {
                    return Ok(None);
                }
                push_responses_call(payload, &mut self.stream_calls, false);
                self.stream_events.push(raw);
                Ok(Some(suppressed_outcome()))
            }
            "response.function_call_arguments.delta" => {
                push_responses_call(payload, &mut self.stream_calls, true);
                self.stream_events.push(raw);
                Ok(Some(suppressed_outcome()))
            }
            "response.output_item.done" => {
                if payload.pointer("/item/type").and_then(Value::as_str) != Some("function_call") {
                    return Ok(None);
                }
                self.stream_events.push(raw);
                Ok(Some(suppressed_outcome()))
            }
            "response.completed" => {
                if self.stream_calls.is_empty() {
                    return Ok(None);
                }
                self.stream_finish.push(raw);
                self.decide_stream_calls()?;
                Ok(Some(suppressed_outcome()))
            }
            _ => Ok(None),
        }
    }

    fn decide_stream_calls(&mut self) -> Result<(), ProxyError> {
        if self.stream_calls.is_empty() {
            return Ok(());
        }
        let proxy = self
            .stream_calls
            .iter()
            .any(|call| PROXY_TOOLS.contains(&call.name.as_str()));
        let harness = self
            .stream_calls
            .iter()
            .any(|call| !PROXY_TOOLS.contains(&call.name.as_str()));
        if !proxy || harness {
            self.forward.extend(self.stream_events.drain(..));
            self.forward.extend(self.stream_finish.drain(..));
            self.stream_calls.clear();
            return Ok(());
        }
        for call in std::mem::take(&mut self.stream_calls) {
            self.record_call(proxy_call(call));
        }
        self.stream_events.clear();
        self.stream_finish.clear();
        self.proxy_round = true;
        Ok(())
    }

    fn record_call(&mut self, call: ProxyCall) {
        let req_body = request_body_mut(&mut self.request);
        let Some(mut contents) = extract_core_contents(self.wire, req_body) else {
            return;
        };
        let text = execute_by_name(
            &self.proxy,
            &self.session_id,
            &self.runtime,
            &mut contents,
            &call,
        );
        write_core_contents(self.wire, req_body, contents);
        self.completed.push(CompletedCall { call, text });
    }
}

fn suppressed_outcome() -> RewriteOutcome {
    RewriteOutcome {
        changed: true,
        empty: true,
    }
}

fn is_stream_end(payload: &Value) -> bool {
    matches!(
        payload.get("type").and_then(Value::as_str),
        Some("message_stop" | "response.completed")
    )
}

fn stream_index(payload: &Value) -> usize {
    payload
        .get("index")
        .or_else(|| payload.get("output_index"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize
}

fn stream_call_mut(calls: &mut Vec<StreamCall>, index: usize) -> &mut StreamCall {
    if let Some(position) = calls.iter().position(|call| call.index == index) {
        return &mut calls[position];
    }
    let position = calls.len();
    calls.push(StreamCall {
        index,
        ..StreamCall::default()
    });
    &mut calls[position]
}

fn arguments_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(value) if !value.is_null() => value.to_string(),
        _ => String::new(),
    }
}

fn collect_openai_calls(payload: &mut Value, calls: &mut Vec<StreamCall>) -> (bool, bool) {
    let mut saw_tool = false;
    let mut finished = false;
    let Some(choices) = payload.get_mut("choices").and_then(Value::as_array_mut) else {
        return (false, false);
    };
    for choice in choices {
        if let Some(delta) = choice.get_mut("delta").and_then(Value::as_object_mut) {
            if let Some(Value::Array(items)) = delta.remove("tool_calls") {
                saw_tool = true;
                for item in items {
                    collect_openai_call(&item, calls);
                }
            }
        }
        if choice
            .get("finish_reason")
            .is_some_and(|value| !value.is_null())
        {
            finished = true;
        }
    }
    (saw_tool, finished)
}

fn collect_openai_call(item: &Value, calls: &mut Vec<StreamCall>) {
    let index = item
        .get("index")
        .and_then(Value::as_u64)
        .unwrap_or(calls.len() as u64) as usize;
    let call = stream_call_mut(calls, index);
    if let Some(id) = item
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        call.id = id.to_string();
    }
    let Some(function) = item.get("function") else {
        return;
    };
    if let Some(name) = function.get("name").and_then(Value::as_str) {
        call.name = name.to_string();
    }
    call.args
        .push_str(&arguments_text(function.get("arguments")));
}

fn push_stream_call(payload: &Value, calls: &mut Vec<StreamCall>, append_args: bool) {
    let block = payload.get("content_block");
    let index = stream_index(payload);
    let call = stream_call_mut(calls, index);
    if let Some(id) = block
        .and_then(|block| block.get("id"))
        .and_then(Value::as_str)
    {
        call.id = id.to_string();
    }
    if let Some(name) = block
        .and_then(|block| block.get("name"))
        .and_then(Value::as_str)
    {
        call.name = name.to_string();
    }
    if append_args {
        let fragment = arguments_text(payload.get("delta").and_then(|d| d.get("partial_json")));
        if call.args == "{}" {
            call.args.clear();
        }
        call.args.push_str(&fragment);
    } else if let Some(input) = block.and_then(|block| block.get("input")) {
        if !input.as_object().is_some_and(serde_json::Map::is_empty) {
            call.args = arguments_text(Some(input));
        }
    }
}

fn push_responses_call(payload: &Value, calls: &mut Vec<StreamCall>, append_args: bool) {
    let index = stream_index(payload);
    let call = stream_call_mut(calls, index);
    if let Some(item) = payload.get("item") {
        if let Some(id) = item.get("call_id").and_then(Value::as_str) {
            call.id = id.to_string();
        }
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            call.name = name.to_string();
        }
        if !append_args {
            call.args = arguments_text(item.get("arguments"));
        }
    }
    if append_args {
        call.args.push_str(&arguments_text(payload.get("delta")));
    }
}

fn proxy_call(call: StreamCall) -> ProxyCall {
    let args = serde_json::from_str(&call.args).unwrap_or_else(|_| json!({}));
    let part = json!({
        "functionCall": {
            "id": call.id,
            "name": call.name,
            "args": args
        }
    });
    ProxyCall {
        name: call.name,
        id: call.id,
        part,
        args,
    }
}
struct RewriteOutcome {
    changed: bool,
    empty: bool,
}

/// Remove every proxy call from one SSE payload. A remaining non-proxy call is
/// a harness call, so this turn ends after the proxy calls are recorded.
fn rewrite_payload(
    payload: &mut Value,
    state: &mut SseState,
) -> Result<RewriteOutcome, ProxyError> {
    let Some(candidates) = candidates_mut(payload) else {
        return Ok(RewriteOutcome {
            changed: false,
            empty: false,
        });
    };
    let mut changed = false;
    let mut empty = true;
    let mut calls = Vec::new();
    let mut harness_id = None;
    for candidate in candidates.iter_mut() {
        let Some(parts) = candidate
            .get_mut("content")
            .and_then(|content| content.get_mut("parts"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        let mut kept = Vec::with_capacity(parts.len());
        let mut swallowed_signature = None;
        for mut part in parts.drain(..) {
            remember_or_apply_step_signature(&mut part, &mut state.step_signature);
            if let Some(call) = proxy_call_from_part(&part) {
                swallowed_signature =
                    swallowed_signature.or_else(|| part.get("thoughtSignature").cloned());
                calls.push(call);
                changed = true;
                continue;
            }
            if harness_id.is_none() {
                harness_id = function_call_id(&part);
            }
            kept.push(part);
        }
        hand_over_signature(&mut kept, swallowed_signature);
        *parts = kept;
        if !parts.is_empty() {
            empty = false;
        }
    }
    if let Some(harness_id) = harness_id {
        state.harness_call_id = Some(harness_id);
    }
    for call in calls {
        state.record_call(call);
    }
    Ok(RewriteOutcome { changed, empty })
}

/// A signed part sets the step's signature; a swallowed call without one
/// takes it, because the store replays that call as a turn of its own.
fn remember_or_apply_step_signature(part: &mut Value, step_signature: &mut Option<Value>) {
    if let Some(signature) = part.get("thoughtSignature") {
        *step_signature = Some(signature.clone());
        return;
    }
    if proxy_call_from_part(part).is_some() {
        if let Some(signature) = step_signature.clone() {
            part["thoughtSignature"] = signature;
        }
    }
}

/// The model signs the first call of a step. If the proxy swallowed that call,
/// the first kept function call inherits the signature.
fn hand_over_signature(kept: &mut [Value], signature: Option<Value>) {
    let Some(signature) = signature else {
        return;
    };
    let first_call = kept
        .iter_mut()
        .find(|part| part.get("functionCall").is_some());
    if let Some(part) = first_call {
        if part.get("thoughtSignature").is_none() {
            part["thoughtSignature"] = signature;
        }
    }
}

fn candidates_mut(payload: &mut Value) -> Option<&mut Vec<Value>> {
    let nested = payload
        .get("response")
        .and_then(|response| response.get("candidates"))
        .is_some();
    if nested {
        return payload
            .get_mut("response")
            .and_then(|response| response.get_mut("candidates"))
            .and_then(Value::as_array_mut);
    }
    payload.get_mut("candidates").and_then(Value::as_array_mut)
}
fn function_call_id(part: &Value) -> Option<String> {
    part.get("functionCall")
        .filter(|call| {
            call.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| !PROXY_TOOLS.contains(&name))
        })
        .and_then(|call| call.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn sse_data(text: &str) -> Option<String> {
    let mut data = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start());
        }
    }
    if data.is_empty() {
        None
    } else {
        Some(data.join("\n"))
    }
}

fn streaming_response(state: SseState) -> Response<Body> {
    let body = Body::from_stream(futures::stream::unfold(state, next_chunk));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(body)
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

async fn next_chunk(mut state: SseState) -> Option<(Result<Bytes, std::io::Error>, SseState)> {
    loop {
        if let Some(bytes) = state.forward.pop_front() {
            return Some((Ok(bytes), state));
        }
        if state.done {
            finish_stream(&mut state);
            return None;
        }
        if let Some(result) = next_buffered(&mut state) {
            match result {
                Ok(Some(bytes)) => return Some((Ok(bytes), state)),
                Ok(None) => continue,
                Err(error) => {
                    state.done = true;
                    return Some((Ok(error_event(&error)), state));
                }
            }
        }
        let polled = match state.stream.as_mut() {
            Some(stream) => timeout(ROUND_TIMEOUT, stream.next()).await,
            None => Ok(None),
        };
        match polled {
            Ok(Some(Ok(chunk))) => state.pending.extend_from_slice(&chunk),
            Ok(Some(Err(error))) => {
                state.done = true;
                return Some((Ok(error_event(&ProxyError::Upstream(error))), state));
            }
            Ok(None) => {
                state.stream = None;
                if !state.pending.is_empty() {
                    state.pending.extend_from_slice(b"\n\n");
                    continue;
                }
                if let Err(error) = state.finish_round() {
                    state.done = true;
                    let message = format!("{error}");
                    publish_error(
                        &state.proxy.inner.events,
                        &state.session_id,
                        message.clone(),
                    );
                    return Some((Ok(Bytes::from(sse_error(&message))), state));
                }
                if !state.done {
                    if let Err(error) = state.follow_up().await {
                        state.done = true;
                        let message = format!("{error}");
                        publish_error(
                            &state.proxy.inner.events,
                            &state.session_id,
                            message.clone(),
                        );
                        return Some((Ok(Bytes::from(sse_error(&message))), state));
                    }
                }
            }
            Err(_) => {
                state.done = true;
                let message = "upstream round timed out".to_string();
                publish_error(
                    &state.proxy.inner.events,
                    &state.session_id,
                    message.clone(),
                );
                return Some((Ok(Bytes::from(sse_error(&message))), state));
            }
        }
    }
}

fn finish_stream(state: &mut SseState) {
    if let Some(usage) = state.usage.take() {
        state
            .runtime
            .last_input_tokens
            .store(usage.input_tokens, Ordering::Relaxed);
        state.proxy.inner.events.publish(SessionEvent::new(
            &state.session_id,
            0,
            EventKind::Usage,
            json!({
                "inputTokens": usage.input_tokens,
                "outputTokens": usage.output_tokens
            }),
        ));
    }
}

/// One SSE event with the blank-line delimiter `take_event` removed.
fn delimited(event: &[u8]) -> Bytes {
    let mut bytes = Vec::with_capacity(event.len() + 2);
    bytes.extend_from_slice(event);
    bytes.extend_from_slice(b"\n\n");
    Bytes::from(bytes)
}

/// The next buffered event, processed; `Ok(None)` swallows it, `None` means
/// nothing complete is buffered.
fn next_buffered(state: &mut SseState) -> Option<Result<Option<Bytes>, ProxyError>> {
    let event = take_event(&mut state.pending)?;
    Some(state.process_event(&event))
}

fn take_event(pending: &mut Vec<u8>) -> Option<Vec<u8>> {
    let (index, delimiter) = find_delimiter(pending)?;
    let event = pending[..index].to_vec();
    pending.drain(..index + delimiter);
    Some(event)
}

fn find_delimiter(pending: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    while index < pending.len() {
        if pending[index..].starts_with(b"\n\n") {
            return Some((index, 2));
        }
        if pending[index..].starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
        index += 1;
    }
    None
}

fn error_event(error: &ProxyError) -> Bytes {
    Bytes::from(sse_error(&error.to_string()))
}

fn sse_error(message: &str) -> String {
    format!("data: {}\n\n", json!({ "error": { "message": message } }))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
