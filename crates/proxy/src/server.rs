use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use api::state::Settings;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, Response, StatusCode, Uri};
use axum::routing::{any, get};
use axum::Router;
use emberloop_core::events::{EventBus, EventKind, SessionEvent};
use emberloop_core::steer::{delivered_event, SteerEntry, SteerQueue, SteerQueues};
use futures::{Stream, StreamExt};
use prompts::{
    render_text, PinnedPrompts, PromptError, PromptStore, PromptVars, NUDGE_EMERGENCY, NUDGE_FIRST,
    NUDGE_PRESSURE, NUDGE_TIER2, NUDGE_TIER3,
};
use serde_json::{json, Value};
use store::{DataRoot, ProjectsStore, SessionsStore};
use tokio::time::timeout;

use crate::compressor::{
    latest_user_turn, CompressCall, CompressPolicy, Compressor, DecompressCall, NativeCompressor,
    SearchHit,
};
use crate::ctx::CtxStore;
use crate::meter;
use crate::nudge::{self, NudgeConfig, NudgeKind, NudgeState};
use crate::splice::{turn_key, Splice, SpliceStore};
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
const MAX_BODY_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const PROXY_TOOLS: &[&str] = &["compress", "decompress", "search_context", "acp_status"];

type ByteStream = std::pin::Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// Absolute upstream origin, e.g. the Cloud Code CCPA endpoint.
    pub upstream_base: String,
    /// HMAC key for Session tokens. The daemon generates one per run.
    pub token_secret: Vec<u8>,
    /// OpenCode Go origin the pi wires (chat, responses, messages) stream to.
    pub opencode_upstream: String,
    /// Replaces every key provider's origin on `x/<id>/…` routes; tests point it at a fake.
    pub provider_origin: Option<String>,
    /// The `execute` tool can reach the code graph, so the skill is worth the tokens.
    pub codebase_memory: bool,
}

impl ProxyConfig {
    pub fn new(token_secret: impl Into<Vec<u8>>) -> Self {
        Self {
            upstream_base: DEFAULT_UPSTREAM.to_string(),
            token_secret: token_secret.into(),
            opencode_upstream: DEFAULT_OPENCODE_UPSTREAM.to_string(),
            provider_origin: None,
            codebase_memory: false,
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

impl ProxyConfig {
    pub fn with_provider_origin(mut self, origin: impl Into<String>) -> Self {
        self.provider_origin = Some(origin.into());
        self
    }

    pub fn with_codebase_memory(mut self, codebase_memory: bool) -> Self {
        self.codebase_memory = codebase_memory;
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("unknown provider {0}")]
    UnknownProvider(String),
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
    #[error("{0}")]
    Encoding(#[from] crate::encoding::Error),
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
    #[error("compression store: {0}")]
    Compression(String),
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
    models: Arc<api::models::ModelCatalogue>,
    events: Arc<EventBus>,
    steers: SteerQueues,
    client: reqwest::Client,
    settings: Arc<tokio::sync::RwLock<Settings>>,
    runtimes: RwLock<HashMap<String, Arc<SessionRuntime>>>,
}

struct SessionRuntime {
    pins: PinnedPrompts,
    splices: Mutex<SpliceStore>,
    ctx: Mutex<CtxStore>,
    compressor: Mutex<Box<dyn Compressor>>,
    last_input_tokens: AtomicU64,
    nudge: Mutex<NudgeState>,
    nudge_file: std::path::PathBuf,
    /// Set by an emergency nudge; while set, every outgoing request truncates its older large
    /// results, so the truncated prefix stays byte-identical. Cleared by a fold or a reset.
    /// In memory only: a restarted proxy starts clear.
    emergency_truncated: AtomicBool,
}

impl SessionRuntime {
    fn new(
        pins: PinnedPrompts,
        spill_dir: std::path::PathBuf,
        splices_file: std::path::PathBuf,
    ) -> Result<Self, ProxyError> {
        let nudge_file = splices_file.with_file_name("nudge.json");
        let ctx_file = splices_file.with_file_name("ctx.jsonl");
        let ctx = CtxStore::open(&spill_dir, ctx_file);
        Ok(Self {
            pins,
            nudge: Mutex::new(nudge::load(&nudge_file)),
            nudge_file,
            splices: Mutex::new(SpliceStore::open(splices_file)),
            ctx: Mutex::new(ctx),
            compressor: Mutex::new(Box::new(
                NativeCompressor::new(spill_dir).map_err(ProxyError::Compression)?,
            )),
            last_input_tokens: AtomicU64::new(0),
            emergency_truncated: AtomicBool::new(false),
        })
    }

    /// A compress or a conversation restart closes the first-nudge gate (ticket 20) and ends
    /// the sticky emergency truncation.
    fn close_nudge_gate(&self) -> Result<(), ProxyError> {
        self.emergency_truncated.store(false, Ordering::Relaxed);
        let mut gate = lock(&self.nudge)?;
        gate.reset_after_compress();
        nudge::save(&self.nudge_file, &gate)?;
        Ok(())
    }

    /// Stores the gate a request observed. Called only once upstream has accepted the
    /// request, so a failed or cancelled one leaves the gate open for the retry.
    fn commit_nudge(&self, observed: NudgeState) -> Result<(), ProxyError> {
        let mut gate = lock(&self.nudge)?;
        if *gate != observed {
            nudge::save(&self.nudge_file, &observed)?;
            *gate = observed;
        }
        Ok(())
    }
}

impl ProxyState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: ProxyConfig,
        sessions: SessionsStore,
        projects: ProjectsStore,
        data_root: DataRoot,
        prompts: Arc<PromptStore>,
        models: Arc<api::models::ModelCatalogue>,
        events: Arc<EventBus>,
        steers: SteerQueues,
        settings: Arc<tokio::sync::RwLock<Settings>>,
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
                models,
                events,
                steers,
                client: reqwest::Client::new(),
                settings,
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
        let spill = self.inner.data_root.session_spill_dir(session_id);
        let mut compressor = lock(&runtime.compressor)?;
        let blocks = spill.join("blocks");
        if blocks.exists() {
            let mut generation = 1;
            while spill.join(format!("blocks-reset-{generation}")).exists() {
                generation += 1;
            }
            std::fs::rename(&blocks, spill.join(format!("blocks-reset-{generation}")))?;
        }
        *compressor = Box::new(NativeCompressor::new(spill).map_err(ProxyError::Compression)?);
        lock(&runtime.splices)?.reset()?;
        runtime.close_nudge_gate()?;
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
            ..Default::default()
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
        let runtime = Arc::new(SessionRuntime::new(pins, spill, splices)?);

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
        .route("/proxy/{token}/{*rest}", any(handler))
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

    let config = &state.inner.config;
    let (base, path, wire) = match provider_route(config, &rest)? {
        Some((origin, path)) => {
            let wire = WireKind::from_rest(&path).map(|(wire, _)| wire);
            (origin, path, wire)
        }
        None => match WireKind::from_rest(&rest) {
            Some((wire, Some(pi_path))) => (config.opencode_upstream.clone(), pi_path, Some(wire)),
            Some((wire, None)) => (config.upstream_base.clone(), rest, Some(wire)),
            None => (config.upstream_base.clone(), rest, None),
        },
    };
    let Some(wire) = wire else {
        return forward_plain(&state, &method, &base, &path, uri.query(), &headers, body).await;
    };
    let encoding = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok());
    let body = crate::encoding::decode(encoding, body, MAX_BODY_BYTES)?;
    handle_generation(
        &state,
        &session_id,
        wire,
        &base,
        &path,
        uri.query(),
        &headers,
        body,
    )
    .await
}

/// `x/<provider>/<path>` -> the provider's origin and `<path>`; `None` for any other route.
fn provider_route(
    config: &ProxyConfig,
    rest: &str,
) -> Result<Option<(String, String)>, ProxyError> {
    let Some(tail) = rest.strip_prefix("x/") else {
        return Ok(None);
    };
    let (id, path) = tail.split_once('/').unwrap_or((tail, ""));
    let provider = emberloop_core::providers::proxy_provider(id)
        .ok_or_else(|| ProxyError::UnknownProvider(id.to_string()))?;
    let origin = config
        .provider_origin
        .clone()
        .unwrap_or_else(|| provider.split().0.to_string());
    Ok(Some((origin, path.to_string())))
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
    base: &str,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response<Body>, ProxyError> {
    let url = upstream_url(base, path, query);
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
    base: &str,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response<Body>, ProxyError> {
    let runtime = state.runtime(session_id).await?;
    let mut request: Value = serde_json::from_slice(&body)?;
    let queue = state.inner.steers.queue(session_id);
    queue.begin_request().await;
    dump_request(session_id, "in", &request);
    let injected = inject_steer(&runtime, wire, &mut request, &queue).await;
    let window = context_window(state, session_id).await;
    let settings = state.inner.settings.read().await.clone();
    let outcome = prepare_request(
        &runtime,
        wire,
        &mut request,
        window,
        &settings,
        state.inner.config.codebase_memory,
    )?;
    let observed_gate = outcome.gate.clone();
    publish_prepare_events(&state.inner.events, session_id, outcome);
    dump_request(session_id, "out", &request);
    let url = upstream_url(base, path, query);
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
    if let Some(gate) = observed_gate.filter(|_| status.is_success()) {
        runtime.commit_nudge(gate)?;
    }
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
            settings,
            window,
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
        settings,
        window,
    )
    .await
}

fn publish_prepare_events(events: &EventBus, session_id: &str, outcome: PrepareOutcome) {
    if let Some(report) = outcome.nudge {
        events.publish(SessionEvent::new(
            session_id,
            0,
            EventKind::Compression,
            json!({
                "action": "nudge",
                "text": format!("nudge: {}", report.kind),
                "kind": report.kind,
                "tokens": report.tokens,
                "sinceLast": report.since_last,
            }),
        ));
    }
    if let Some(text) = outcome.preflight {
        events.publish(SessionEvent::new(
            session_id,
            0,
            EventKind::Compression,
            json!({ "action": "preflight", "text": text }),
        ));
    }
    if let Some(text) = outcome.unmatched {
        events.publish(SessionEvent::new(
            session_id,
            0,
            EventKind::Compression,
            json!({ "action": "fold-miss", "text": text }),
        ));
    }
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
    settings: Settings,
    window: usize,
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
        let (turns, after) = run_proxy_tool(
            state,
            session_id,
            &runtime,
            wire,
            &mut request,
            call,
            &settings,
            window,
        )?;
        store_proxy_splice(&runtime, &turns, after)?;
    }

    let Some((status, content_type, bytes)) = final_response else {
        return Err(ProxyError::ToolRoundLimit);
    };
    publish_usage(state, session_id, &runtime, &bytes, wire, &request).await;
    Ok(response_with(status, content_type.as_deref(), bytes))
}

struct PrepareOutcome {
    nudge: Option<NudgeReport>,
    /// The nudge gate as this request left it; `handle_generation` commits it on success.
    gate: Option<NudgeState>,
    preflight: Option<String>,
    /// Why no stored fold matched, when one was expected: the session log's only trace.
    unmatched: Option<String>,
}

fn prepare_request(
    runtime: &SessionRuntime,
    wire: WireKind,
    request: &mut Value,
    window: usize,
    settings: &Settings,
    codebase_memory: bool,
) -> Result<PrepareOutcome, ProxyError> {
    let request = request_body_mut(request);

    if settings.proxy_compression && settings.inject_tool {
        inject_tools_for_wire(wire, request, &runtime.pins);
    }
    inject_prompts_for_wire(wire, request, &runtime.pins, codebase_memory);
    let Some(mut contents) = extract_core_contents(wire, request) else {
        return Ok(PrepareOutcome {
            nudge: None,
            gate: None,
            preflight: None,
            unmatched: None,
        });
    };
    // Arrival replacement runs first: a fold stores the `ctx` handle, so a later request must
    // carry the handle again before the fold's stored turns can be matched.
    lock(&runtime.ctx)?.apply(&mut contents);
    // A fold was chosen over the history as the model saw it, steers and proxy pairs included, so
    // its stored turns include them. The harness resends its own history without them: the block
    // is looked for ignoring the turns the splice store would put back.
    let spliced = lock(&runtime.splices)?.turns();
    let folded = lock(&runtime.compressor)?.apply_skipping(&mut contents, &spliced);
    let unmatched = if folded == 0 {
        lock(&runtime.compressor)?.fold_report(&contents, &spliced)
    } else {
        None
    };
    lock(&runtime.splices)?.apply(&mut contents)?;
    if wire == WireKind::Google {
        crate::splice::sign_unsigned_calls(&mut contents);
    }
    strip_forged_markers(&mut contents);
    let preflight = if settings.proxy_compression {
        preflight_fold(runtime, &mut contents, window, settings)?
    } else {
        None
    };
    if preflight.is_some() {
        runtime.emergency_truncated.store(false, Ordering::Relaxed);
    }
    // The nudge goes last, on the final contents, so its turn numbers match what compress sees.
    let (nudge, gate) = if settings.proxy_compression && settings.inject_nudge {
        inject_nudge(runtime, &mut contents, window, settings)?
    } else {
        (None, None)
    };
    if nudge
        .as_ref()
        .is_some_and(|report| report.kind == NudgeKind::Emergency.name())
    {
        runtime.emergency_truncated.store(true, Ordering::Relaxed);
    }
    if runtime.emergency_truncated.load(Ordering::Relaxed) {
        truncate_emergency(&mut contents);
    }
    write_core_contents(wire, request, contents);
    Ok(PrepareOutcome {
        nudge,
        gate,
        preflight,
        unmatched,
    })
}

fn compress_policy(settings: &Settings) -> CompressPolicy {
    CompressPolicy {
        preserve_recent_messages: settings.preserve_recent_messages as usize,
        protected_tools: settings.protected_tools.clone(),
    }
}

/// The reference's preflight backstop: input alone over the window is folded before
/// forwarding. The model still chose nothing here; the proxy writes the summary and the
/// originals stay in the spill dir for `decompress`.
fn preflight_fold(
    runtime: &SessionRuntime,
    contents: &mut Vec<Value>,
    window: usize,
    settings: &Settings,
) -> Result<Option<String>, ProxyError> {
    if window == 0 {
        return Ok(None);
    }
    let policy = compress_policy(settings);
    let mut folded = None;
    for _ in 0..4 {
        if estimate_tokens(contents) <= u64::try_from(window).unwrap_or(u64::MAX) {
            break;
        }
        let keep = policy.preserve_recent_messages.min(contents.len());
        // Never reaches the latest user message: no fold may include it.
        let end = contents
            .len()
            .saturating_sub(keep)
            .min(latest_user_turn(contents).unwrap_or(usize::MAX));
        if end <= 1 {
            break;
        }
        let call = CompressCall {
            start: 1,
            end,
            summary: format!(
                "Preflight fold: the request exceeded the context window, so the proxy folded turns 1–{} automatically. The originals are in the session spill dir and `decompress` restores them.",
                end - 1
            ),
            topic: Some("preflight".to_string()),
        };
        let mut compressor = lock(&runtime.compressor)?;
        match compressor.compress_batch(contents, vec![call], &policy) {
            Ok(text) => folded = Some(text),
            Err(_) => break,
        }
    }
    Ok(folded)
}

fn strip_forged_markers(contents: &mut [Value]) {
    for turn in contents.iter_mut() {
        let role = turn.get("role").and_then(Value::as_str).unwrap_or_default();
        if role != "model" && role != "assistant" {
            continue;
        }
        let mut cleaned = false;
        let Some(parts) = turn.get_mut("parts").and_then(Value::as_array_mut) else {
            continue;
        };
        for part in parts {
            let Some(text) = part.get("text").and_then(Value::as_str) else {
                continue;
            };
            if !text.starts_with("📦 [acp]") && !text.starts_with("❌ [acp]") {
                continue;
            }
            let kept = text
                .lines()
                .filter(|line| !line.starts_with("📦 [acp]") && !line.starts_with("❌ [acp]"))
                .collect::<Vec<_>>()
                .join("\n");
            part["text"] = json!(kept.trim());
            cleaned = true;
        }
        if cleaned {
            if let Some(raw) = turn.get_mut("_raw") {
                strip_raw_markers(raw);
            }
        }
    }
}

fn strip_raw_markers(raw: &mut Value) {
    if let Some(content) = raw.get("content").and_then(Value::as_str) {
        let kept = content
            .lines()
            .filter(|line| !line.starts_with("📦 [acp]") && !line.starts_with("❌ [acp]"))
            .collect::<Vec<_>>()
            .join("\n");
        raw["content"] = json!(kept.trim());
    }
    if let Some(blocks) = raw.get_mut("content").and_then(Value::as_array_mut) {
        for block in blocks {
            let Some(text) = block.get("text").and_then(Value::as_str) else {
                continue;
            };
            if !text.starts_with("📦 [acp]") && !text.starts_with("❌ [acp]") {
                continue;
            }
            block["text"] = json!(text
                .lines()
                .filter(|line| {
                    !line.starts_with("📦 [acp]") && !line.starts_with("❌ [acp]")
                })
                .collect::<Vec<_>>()
                .join("\n")
                .trim());
        }
    }
}

fn truncate_emergency(contents: &mut [Value]) {
    for turn in contents.iter_mut() {
        let is_summary = turn
            .get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts.iter().any(|part| {
                    part.get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.starts_with("📦 [acp]"))
                })
            });
        if is_summary {
            continue;
        }
        let mut truncated_raw = None;
        let Some(parts) = turn.get_mut("parts").and_then(Value::as_array_mut) else {
            continue;
        };
        for part in parts {
            let Some(response) = part.get("functionResponse") else {
                continue;
            };
            let Some(result) = response
                .get("response")
                .and_then(|response| response.get("result"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if result.len() <= 4_000 {
                continue;
            }
            let head: String = result.chars().take(2_000).collect();
            let tail: String = result
                .chars()
                .rev()
                .take(1_000)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            let truncated = format!(
                "{head}\n\n[... emergency truncation: {} chars omitted from the visible tool output; the stored history and spill copy are unchanged ...]\n\n{tail}",
                result.len() - head.len() - tail.len()
            );
            let id = response
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(slot) = part
                .get_mut("functionResponse")
                .and_then(|response| response.get_mut("response"))
                .and_then(|response| response.get_mut("result"))
            {
                *slot = json!(&truncated);
            }
            truncated_raw = Some((id, truncated));
        }
        if let Some((id, text)) = truncated_raw {
            if let Some(raw) = turn.get_mut("_raw") {
                set_raw_tool_result(raw, &id, &text);
            }
        }
    }
}

fn set_raw_tool_result(raw: &mut Value, id: &str, text: &str) {
    if raw.get("tool_call_id").and_then(Value::as_str) == Some(id) {
        raw["content"] = json!(text);
        return;
    }
    let Some(blocks) = raw.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        if block.get("tool_use_id").and_then(Value::as_str) == Some(id) {
            block["content"] = json!(text);
        }
    }
}

#[derive(Debug)]
pub struct NudgeReport {
    pub kind: &'static str,
    pub tokens: u64,
    pub since_last: u64,
}

async fn context_window(state: &ProxyState, session_id: &str) -> usize {
    match state.inner.sessions.get(session_id).await {
        Some(meta) => state.inner.models.window(&meta.model).await.unwrap_or(0),
        None => 0,
    }
}

/// Tokens by the chars / 4 rule the reference uses, over the serialised turns.
fn estimate_tokens(turns: &[Value]) -> u64 {
    turns
        .iter()
        .map(|turn| u64::try_from(turn.to_string().len() / 4).unwrap_or(u64::MAX))
        .sum()
}

fn kilo(tokens: u64) -> String {
    format!("{:.1}K", tokens as f64 / 1000.0)
}

struct CompressibleView {
    tokens: u64,
    breakdown: String,
    ranges: String,
}

fn compressible_view(turns: &[Value], preserve_recent_messages: usize) -> CompressibleView {
    let end = turns.len().saturating_sub(preserve_recent_messages);
    let (tokens, count) = if end > 1 {
        (estimate_tokens(&turns[1..end]), end - 1)
    } else {
        (0, 0)
    };
    let ranges = if count == 0 {
        "Compressible ranges (1, oldest first): none".to_string()
    } else {
        format!(
            "Compressible ranges (1, oldest first):\n  turns 1–{}  {count} turns  {} tokens",
            end - 1,
            kilo(tokens)
        )
    };
    CompressibleView {
        tokens,
        breakdown: format!(
            "Context breakdown: {} tool | {} text",
            kilo(tokens),
            turns.len() - end
        ),
        ranges,
    }
}

/// Decides on the first or next nudge and, when one fires, puts it into the outgoing contents
/// right after the newest tool result. It is transient: no splice stores it, so a later request
/// carries it only when the gate fires again. Runs on the final contents, after the compressor
/// and the splices, so its turn numbers are the ones `compress` will use. A turn without a tool
/// result gets no nudge. The gate is decided on a copy and returned for the caller to commit once
/// upstream accepts the request.
fn inject_nudge(
    runtime: &SessionRuntime,
    contents: &mut Vec<Value>,
    window: usize,
    settings: &Settings,
) -> Result<(Option<NudgeReport>, Option<NudgeState>), ProxyError> {
    let Some(result_index) = last_tool_result_index(contents) else {
        return Ok((None, None));
    };
    let preserve = settings.preserve_recent_messages as usize;
    let view = compressible_view(contents, preserve);
    let tokens = match runtime.last_input_tokens.load(Ordering::Relaxed) {
        0 => estimate_tokens(contents),
        used => used,
    };
    let active_blocks = lock(&runtime.compressor)?.status().active_blocks as u64;
    let config = NudgeConfig {
        max_context_limit_pct: u64::from(settings.max_context_limit),
        emergency_threshold_pct: u64::from(settings.emergency_threshold_percent),
        min_pressure_benefit_tokens: u64::from(settings.min_pressure_benefit_tokens),
        tier2_trigger: u64::from(settings.tier2_trigger),
        tier3_trigger: u64::from(settings.tier3_trigger),
    };
    let mut gate = lock(&runtime.nudge)?.clone();
    let fired = gate.observe_with(
        tokens,
        u64::try_from(window).unwrap_or(0),
        view.tokens,
        active_blocks,
        &config,
    );
    let Some(fired) = fired else {
        return Ok((None, Some(gate)));
    };
    let id = match fired.kind {
        NudgeKind::First => NUDGE_FIRST,
        NudgeKind::Pressure => NUDGE_PRESSURE,
        NudgeKind::Emergency => NUDGE_EMERGENCY,
        NudgeKind::Tier2 => NUDGE_TIER2,
        NudgeKind::Tier3 => NUDGE_TIER3,
    };
    let vars = PromptVars {
        context_breakdown: view.breakdown,
        since_last_nudge: format!("+{} since last nudge", kilo(fired.since_last)),
        compressible_ranges: view.ranges,
        ..PromptVars::default()
    };
    let raw = runtime.pins.text(id).unwrap_or_default();
    let text = render_text(id, raw, &vars)?;
    // `_nudge` marks the turn as proxy-only: a fold must not store it, and no wire may see it.
    contents.insert(
        result_index + 1,
        json!({ "role": "user", "parts": [{ "text": text }], "_nudge": true }),
    );
    let report = NudgeReport {
        kind: fired.kind.name(),
        tokens,
        since_last: fired.since_last,
    };
    Ok((Some(report), Some(gate)))
}

/// Index of the newest turn that carries a tool result: the anchor a nudge rides after.
fn last_tool_result_index(contents: &[Value]) -> Option<usize> {
    contents.iter().rposition(|turn| {
        turn.get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts
                    .iter()
                    .any(|part| part.get("functionResponse").is_some())
            })
    })
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
        after_turn: None,
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
#[allow(clippy::too_many_arguments)]
fn run_proxy_tool(
    state: &ProxyState,
    session_id: &str,
    runtime: &SessionRuntime,
    wire: WireKind,
    request: &mut Value,
    call: ProxyCall,
    settings: &Settings,
    window: usize,
) -> Result<(Vec<Value>, Option<String>), ProxyError> {
    let request = request_body_mut(request);
    let mut contents = extract_core_contents(wire, request).ok_or(ProxyError::NoContents)?;
    // Read before the tool runs: a `compress` in this call rewrites the contents it would be read from.
    let after = splice_anchor(&contents);
    let text = execute_by_name(
        state,
        session_id,
        runtime,
        &mut contents,
        &call,
        settings,
        window,
    );
    let turns = proxy_pair(&call, &text, &format!("emberloop-splice:{}", call.id));
    contents.extend(turns.clone());
    write_core_contents(wire, request, contents);
    Ok((turns, after))
}

/// The turn a proxy pair follows: the newest turn of the request that the harness resends. The
/// transient nudge is skipped, because no later request carries it.
fn splice_anchor(contents: &[Value]) -> Option<String> {
    contents
        .iter()
        .rev()
        .find(|turn| turn.get("_nudge").is_none())
        .and_then(turn_key)
}

fn execute_by_name(
    state: &ProxyState,
    session_id: &str,
    runtime: &SessionRuntime,
    contents: &mut Vec<Value>,
    call: &ProxyCall,
    settings: &Settings,
    window: usize,
) -> String {
    match execute_tool(runtime, contents, call, settings, window) {
        Ok(text) => {
            if matches!(call.name.as_str(), "compress" | "decompress") {
                publish_compression_event(&state.inner.events, session_id, runtime, call, &text);
            }
            format!("📦 [acp] {}\n{text}", call.name)
        }
        Err(error) => format!("❌ [acp] {}\nerror: {error}", call.name),
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

fn store_proxy_splice(
    runtime: &SessionRuntime,
    turns: &[Value],
    after: Option<String>,
) -> Result<String, ProxyError> {
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
    let splice = Splice::new(&anchor, turns.to_vec())
        .with_marker(marker)
        .with_after_turn(after);
    lock(&runtime.splices)?.insert(splice)?;
    Ok(anchor)
}

fn execute_tool(
    runtime: &SessionRuntime,
    contents: &mut Vec<Value>,
    call: &ProxyCall,
    settings: &Settings,
    window: usize,
) -> Result<String, String> {
    match call.name.as_str() {
        "compress" => {
            let calls = parse_compress_calls(&call.args)?;
            let mut compressor = lock(&runtime.compressor).map_err(|error| error.to_string())?;
            let folded = compressor.compress_batch(contents, calls, &compress_policy(settings))?;
            // The next nudge must judge the folded history, not the usage from before the fold.
            runtime
                .last_input_tokens
                .store(0, std::sync::atomic::Ordering::Relaxed);
            runtime
                .close_nudge_gate()
                .map_err(|error| error.to_string())?;
            Ok(folded)
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
        "acp_status" => {
            let status = lock(&runtime.compressor)
                .map_err(|error| error.to_string())?
                .status();
            Ok(acp_status_text(contents, window, &status, settings))
        }
        other => Err(format!("not a proxy tool: {other}")),
    }
}

fn acp_status_text(
    contents: &[Value],
    window: usize,
    status: &crate::compressor::CompressorStatus,
    settings: &Settings,
) -> String {
    let view = compressible_view(contents, settings.preserve_recent_messages as usize);
    format_acp_status(
        estimate_tokens(contents),
        window,
        status.active_blocks,
        status.blocks,
        status.spilled_turns,
        status.tokens_compressed,
        status.restored_blocks,
        &view.ranges,
    )
}

#[allow(clippy::too_many_arguments)]
fn format_acp_status(
    tokens: u64,
    window: usize,
    active_blocks: usize,
    total_blocks: usize,
    stored_messages: usize,
    tokens_compressed: u64,
    restored: usize,
    ranges: &str,
) -> String {
    let percent = if window == 0 {
        0
    } else {
        tokens.saturating_mul(100) / u64::try_from(window).unwrap_or(u64::MAX)
    };
    format!(
        "ACP STATUS\nContext: {} / {} tokens ({}%)\nBlocks: {} active, {} total; {} stored messages; {} tokens compressed; {} restored\n{}",
        kilo(tokens),
        kilo(u64::try_from(window).unwrap_or(u64::MAX)),
        percent,
        active_blocks,
        total_blocks,
        stored_messages,
        kilo(tokens_compressed),
        restored,
        ranges
    )
}

/// `compress` takes `ranges: [{start, end, summary, topic?}, ...]`; the single-range form
/// (`start`, `end`, `summary`, `topic` at the top level) is still accepted.
fn parse_compress_calls(args: &Value) -> Result<Vec<CompressCall>, String> {
    let Some(ranges) = args.get("ranges") else {
        return parse_compress_range(args).map(|call| vec![call]);
    };
    let ranges = ranges
        .as_array()
        .ok_or_else(|| "ranges must be an array".to_string())?;
    if ranges.is_empty() {
        return Err("ranges must not be empty".into());
    }
    ranges
        .iter()
        .enumerate()
        .map(|(index, range)| {
            parse_compress_range(range).map_err(|error| format!("range {}: {error}", index + 1))
        })
        .collect()
}

fn parse_compress_range(args: &Value) -> Result<CompressCall, String> {
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
    let encoding = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok());
    let bytes = crate::encoding::encode(encoding, bytes)?;
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

/// Copy the client headers onto a generation request, minus the ones the proxy
/// sets itself: `send_stream` owns `content-type` and a second copy makes
/// upstream answer `Unsupported content type`.
fn with_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        if is_hop_by_hop(name.as_str()) || *name == header::CONTENT_TYPE {
            continue;
        }
        request = request.header(name.as_str(), value.as_bytes());
    }
    request
}

async fn publish_usage(
    state: &ProxyState,
    session_id: &str,
    runtime: &SessionRuntime,
    bytes: &Bytes,
    wire: WireKind,
    request: &Value,
) {
    let Some(usage) = meter::extract_usage(bytes) else {
        return;
    };
    runtime
        .last_input_tokens
        .store(usage.input_tokens, Ordering::Relaxed);
    publish_usage_event(
        state,
        session_id,
        usage,
        crate::context::ContextBreakdown::from_request(wire, request),
    )
    .await;
}

/// The meter reads the window from the live catalogue, so a refreshed window
/// reaches the UI with the next response instead of a stale model snapshot.
async fn publish_usage_event(
    state: &ProxyState,
    session_id: &str,
    usage: meter::Usage,
    breakdown: crate::context::ContextBreakdown,
) {
    let window = match state.inner.sessions.get(session_id).await {
        Some(meta) => state.inner.models.window(&meta.model).await,
        None => None,
    };
    let mut data = json!({
        "inputTokens": usage.input_tokens,
        "outputTokens": usage.output_tokens,
        "cachedTokens": usage.cached_tokens,
        "breakdown": breakdown
    });
    if let Some(window) = window {
        data["contextWindow"] = json!(window);
    }
    state
        .inner
        .events
        .publish(SessionEvent::new(session_id, 0, EventKind::Usage, data));
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
        ProxyError::BodyTooLarge | ProxyError::Encoding(crate::encoding::Error::TooLarge) => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        ProxyError::Encoding(_) => StatusCode::BAD_REQUEST,
        ProxyError::UnknownProvider(_) => StatusCode::NOT_FOUND,
        ProxyError::Prompt(_)
        | ProxyError::ToolRoundLimit
        | ProxyError::Timeout
        | ProxyError::Splice(_)
        | ProxyError::Compression(_)
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
    /// The usage last published for this stream, so a chunk and its events do not publish twice.
    published: Option<meter::Usage>,
    /// The signature of the step being streamed: the model signs its first call.
    step_signature: Option<Value>,
    /// Tool calls a pi wire streams in fragments; the wire decides run or hide.
    stream_calls: Vec<StreamCall>,
    stream_events: Vec<Bytes>,
    stream_finish: Vec<Bytes>,
    proxy_round: bool,
    settings: Settings,
    window: usize,
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
        settings: Settings,
        window: usize,
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
            published: None,
            step_signature: None,
            stream_calls: Vec::new(),
            stream_events: Vec::new(),
            stream_finish: Vec::new(),
            proxy_round: false,
            settings,
            window,
        }
    }

    /// Publish usage as soon as its bytes arrive: a harness may stop reading the stream
    /// right after the answer, and then a publish deferred to the stream end never runs.
    async fn capture_usage(&mut self, bytes: &[u8]) {
        let Some(usage) = meter::extract_usage(bytes) else {
            return;
        };
        if self.published == Some(usage) {
            return;
        }
        self.published = Some(usage);
        self.runtime
            .last_input_tokens
            .store(usage.input_tokens, Ordering::Relaxed);
        publish_usage_event(
            &self.proxy,
            &self.session_id,
            usage,
            crate::context::ContextBreakdown::from_request(self.wire, &self.request),
        )
        .await;
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
        self.published = None;
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
        let req_body = request_body_mut(&mut self.request);
        let contents = extract_core_contents(self.wire, req_body);
        let after = contents.as_deref().and_then(splice_anchor);
        let splice = Splice::new(&anchor, turns.clone())
            .with_marker(&marker)
            .with_after_turn(after);
        lock(&self.runtime.splices)?.insert(splice)?;
        if let Some(mut contents) = contents {
            contents.extend(turns);
            write_core_contents(self.wire, req_body, contents);
        }
        if self.rounds >= MAX_TOOL_ROUNDS {
            return Err(ProxyError::ToolRoundLimit);
        }
        self.completed.clear();
        Ok(())
    }

    async fn process_event(&mut self, event: &[u8]) -> Result<Option<Bytes>, ProxyError> {
        let raw = delimited(event);
        let text = String::from_utf8_lossy(event);
        let Some(data) = sse_data(&text) else {
            return Ok(Some(raw));
        };
        let Ok(mut payload) = serde_json::from_str::<Value>(&data) else {
            return Ok((!self.proxy_round || data.trim() != "[DONE]").then_some(raw));
        };
        self.capture_usage(event).await;
        if !self.proxy_round && self.wire != WireKind::Google {
            if let Some(text) = meter::thinking_delta(&payload) {
                self.proxy.inner.events.publish(SessionEvent::new(
                    &self.session_id,
                    0,
                    EventKind::ThinkingChunk,
                    json!({ "text": text }),
                ));
            }
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
            &self.settings,
            self.window,
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
            return None;
        }
        if let Some(result) = next_buffered(&mut state).await {
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
            Ok(Some(Ok(chunk))) => {
                state.capture_usage(&chunk).await;
                state.pending.extend_from_slice(&chunk);
            }
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

/// One SSE event with the blank-line delimiter `take_event` removed.
fn delimited(event: &[u8]) -> Bytes {
    let mut bytes = Vec::with_capacity(event.len() + 2);
    bytes.extend_from_slice(event);
    bytes.extend_from_slice(b"\n\n");
    Bytes::from(bytes)
}

/// The next buffered event, processed; `Ok(None)` swallows it, `None` means
/// nothing complete is buffered.
async fn next_buffered(state: &mut SseState) -> Option<Result<Option<Bytes>, ProxyError>> {
    let event = take_event(&mut state.pending)?;
    Some(state.process_event(&event).await)
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

fn sse_error(message: &str) -> String {
    format!("data: {}\n\n", json!({ "error": { "message": message } }))
}

fn error_event(error: &ProxyError) -> Bytes {
    Bytes::from(sse_error(&error.to_string()))
}

fn dump_request(session_id: &str, stage: &str, request: &Value) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let Some(dir) = std::env::var_os("EMBERLOOP_PROXY_DUMP") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let body = request.to_string();
    let _ = std::fs::write(dir.join(format!("{session_id}.{stage}.json")), &body);
    // A rotating copy per request: a fold that matched once and then stopped needs both bodies.
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let _ = std::fs::write(
        dir.join(format!("{session_id}.{stage}.{seq:03}.json")),
        &body,
    );
    if seq % 20 == 19 && stage == "in" {
        prune_dumps(&dir, session_id, seq);
    }
}

fn prune_dumps(dir: &std::path::Path, session_id: &str, seq: u64) {
    let cutoff = seq.saturating_sub(60);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix(&format!("{session_id}.")) else {
            continue;
        };
        let Some(number) = rest.trim_end_matches(".json").rsplit('.').next() else {
            continue;
        };
        let Ok(number) = number.parse::<u64>() else {
            continue;
        };
        if number < cutoff {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
