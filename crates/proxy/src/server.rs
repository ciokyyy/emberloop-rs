use std::borrow::Cow;
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
    render_text, PinnedPrompts, PromptError, PromptStore, PromptVars, ACP_STATUS_REPORT,
    NUDGE_EMERGENCY, NUDGE_FIRST, NUDGE_PRESSURE, NUDGE_TIER2, NUDGE_TIER3,
};
use serde_json::{json, Value};
use store::{DataRoot, ProjectsStore, SessionsStore};
use tokio::time::timeout;

use crate::carry;
use crate::compressor::{
    foldable_runs, latest_user_turn, CompressCall, CompressPolicy, Compressor, DecompressCall,
    NativeCompressor,
};
use crate::ctx::CtxStore;
use crate::marker;
use crate::meter;
use crate::nudge::{self, NudgeConfig, NudgeKind, NudgeState};
use crate::review::{self, ReviewTools};
use crate::splice::{turn_key, Splice, SpliceStore};
use crate::token::{TokenError, TokenSigner};
use crate::tokens::turns_tokens;
use crate::turnkeys;
use crate::wire::{
    anthropic_model_turn, extract_core_contents, extract_proxy_call_from_payload,
    inject_prompts_for_wire, inject_tools_for_wire, write_core_contents, AnthropicBlocks, WireKind,
};
pub const DEFAULT_UPSTREAM: &str = "https://daily-cloudcode-pa.googleapis.com";
/// Unverified with a real key: docs.opencode.ai names `/inference/go`, third-party guides `/zen/go/v1`.
pub const DEFAULT_OPENCODE_UPSTREAM: &str = "https://opencode.ai/inference/go";
pub const DEFAULT_CLAUDE_UPSTREAM: &str = "https://api.anthropic.com";
const DEFAULT_ROUND_TIMEOUT: Duration = Duration::from_secs(120);
/// Extra tries after a connect failure or a gateway status, each waiting one more step.
const SEND_RETRIES: u32 = 2;
const DEFAULT_RETRY_BACKOFF: Duration = Duration::from_millis(500);
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
    /// Origin the `claude/<path>` route forwards to; tests point it at a fake.
    pub claude_upstream: String,
    /// Replaces every key provider's origin on `x/<id>/…` routes; tests point it at a fake.
    pub provider_origin: Option<String>,
    /// The `execute` tool can reach the code graph, so the skill is worth the tokens.
    pub codebase_memory: bool,
    /// Runs the background skill review's tool calls; the daemon injects it.
    pub review_tools: Option<Arc<dyn ReviewTools>>,
    /// How long one upstream round may take to answer or to produce its next chunk.
    pub round_timeout: Duration,
    /// The wait before the first retry of a failed send; the next waits twice as long.
    pub retry_backoff: Duration,
}

impl ProxyConfig {
    pub fn new(token_secret: impl Into<Vec<u8>>) -> Self {
        Self {
            upstream_base: DEFAULT_UPSTREAM.to_string(),
            token_secret: token_secret.into(),
            opencode_upstream: DEFAULT_OPENCODE_UPSTREAM.to_string(),
            claude_upstream: DEFAULT_CLAUDE_UPSTREAM.to_string(),
            provider_origin: None,
            codebase_memory: false,
            review_tools: None,
            round_timeout: DEFAULT_ROUND_TIMEOUT,
            retry_backoff: DEFAULT_RETRY_BACKOFF,
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
    pub fn with_claude_upstream(mut self, origin: impl Into<String>) -> Self {
        self.claude_upstream = origin.into();
        self
    }

    pub fn with_provider_origin(mut self, origin: impl Into<String>) -> Self {
        self.provider_origin = Some(origin.into());
        self
    }

    pub fn with_codebase_memory(mut self, codebase_memory: bool) -> Self {
        self.codebase_memory = codebase_memory;
        self
    }

    /// The executor that runs the background skill review's tool calls. Without it no review runs.
    pub fn with_review_tools(mut self, tools: Arc<dyn ReviewTools>) -> Self {
        self.review_tools = Some(tools);
        self
    }

    pub fn with_round_timeout(mut self, round_timeout: Duration) -> Self {
        self.round_timeout = round_timeout;
        self
    }

    pub fn with_retry_backoff(mut self, retry_backoff: Duration) -> Self {
        self.retry_backoff = retry_backoff;
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
    #[error("upstream answered {0}")]
    UpstreamStatus(String),
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
    /// Frozen skill catalog and pinned reviewer instructions, read once per Session runtime.
    catalog: String,
    reviewer: bool,
    /// Set by an emergency nudge; while set, every outgoing request truncates its older large
    /// results, so the truncated prefix stays byte-identical. Cleared by a fold or a reset.
    /// In memory only: a restarted proxy starts clear.
    emergency_truncated: AtomicBool,
    review: Mutex<review::ReviewState>,
    /// One line of usage per review round, so the cache behaviour of the fork can be audited.
    review_log: std::path::PathBuf,
    /// Core turns of the last request the model saw, so a new harness conversation can carry them.
    last_history: Mutex<Arc<Vec<Value>>>,
    /// History carried from an earlier harness conversation; leads every request until the next
    /// reset. In memory only: shortcut, a daemon restart between a switch and its first turn loses it.
    carried: Mutex<Vec<Value>>,
    /// The last miss reported, so an unchanged miss is not published on every request.
    last_fold_miss: Mutex<Option<String>>,
}

impl SessionRuntime {
    fn new(
        pins: PinnedPrompts,
        spill_dir: std::path::PathBuf,
        splices_file: std::path::PathBuf,
    ) -> Result<Self, ProxyError> {
        let nudge_file = splices_file.with_file_name("nudge.json");
        let ctx_file = splices_file.with_file_name("ctx.jsonl");
        let catalog = std::fs::read_to_string(splices_file.with_file_name("skill-catalog.md"))
            .unwrap_or_default();
        let review_log = splices_file.with_file_name("review.jsonl");
        let ctx = CtxStore::open(&spill_dir, ctx_file);
        Ok(Self {
            pins,
            nudge: Mutex::new(nudge::load(&nudge_file)),
            nudge_file,
            catalog,
            reviewer: false,
            splices: Mutex::new(SpliceStore::open(splices_file)),
            ctx: Mutex::new(ctx),
            compressor: Mutex::new(Box::new(
                NativeCompressor::new(spill_dir).map_err(ProxyError::Compression)?,
            )),
            last_input_tokens: AtomicU64::new(0),
            emergency_truncated: AtomicBool::new(false),
            review: Mutex::new(review::ReviewState::default()),
            review_log,
            last_history: Mutex::new(Arc::new(Vec::new())),
            carried: Mutex::new(Vec::new()),
            last_fold_miss: Mutex::new(None),
        })
    }

    /// A compress or a conversation restart clears the tier flags (ticket 20) and ends the
    /// sticky emergency truncation. The first-nudge gate stays shut, so the next fold waits
    /// for the growth step instead of firing on the step after the fold.
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

    /// The Prompt Store the proxy renders from, shared with the daemon's own prompts.
    pub fn prompts(&self) -> Arc<PromptStore> {
        Arc::clone(&self.inner.prompts)
    }

    /// The context window of the Session's model, `None` when the catalogue does not know it.
    pub async fn context_window(&self, session_id: &str) -> Option<usize> {
        let meta = self.inner.sessions.get(session_id).await?;
        self.inner.models.window(&meta.model).await
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
        NativeCompressor::archive_blocks(&spill)?;
        *compressor = Box::new(NativeCompressor::new(spill).map_err(ProxyError::Compression)?);
        let last = std::mem::take(&mut *lock(&runtime.last_history)?);
        if !last.is_empty() {
            *lock(&runtime.carried)? = carry::sanitize(Arc::unwrap_or_clone(last));
        }
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
        let level = self
            .inner
            .settings
            .read()
            .await
            .ponytail_default_level
            .clone();
        let vars = PromptVars {
            project_name: project.as_ref().map(|p| p.name.clone()).unwrap_or_default(),
            project_root: project
                .as_ref()
                .map(|p| p.root.to_string_lossy().to_string())
                .unwrap_or_default(),
            level,
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
        let mut runtime = SessionRuntime::new(pins, spill, splices)?;
        if let Some(reviewer) = &meta.reviewer {
            runtime.reviewer = true;
            let id = if reviewer.role == "spec" {
                prompts::REVIEWER_SPEC
            } else {
                prompts::REVIEWER_STANDARDS
            };
            runtime.catalog.push_str("\n\n");
            runtime
                .catalog
                .push_str(runtime.pins.text(id).unwrap_or_default());
        }
        let runtime = Arc::new(runtime);

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
    let (base, path, wire) = if let Some(path) = rest.strip_prefix("claude/") {
        // The Claude CLI's own credential passes through; only the generation path is a wire.
        let wire = (path == "v1/messages").then_some(WireKind::Anthropic);
        (config.claude_upstream.clone(), path.to_string(), wire)
    } else {
        match provider_route(config, &rest)? {
            Some((origin, path)) => {
                let wire = WireKind::from_rest(&path).map(|(wire, _)| wire);
                (origin, path, wire)
            }
            None => match WireKind::from_rest(&rest) {
                Some((wire, Some(pi_path))) => {
                    (config.opencode_upstream.clone(), pi_path, Some(wire))
                }
                Some((wire, None)) => (config.upstream_base.clone(), rest, Some(wire)),
                None => (config.upstream_base.clone(), rest, None),
            },
        }
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
    let nudge_text = outcome.nudge.as_ref().map(|report| report.text.clone());
    publish_prepare_events(&state.inner.events, session_id, outcome);
    dump_request(session_id, "out", &request);
    let url = upstream_url(base, path, query);
    let sent = outgoing_body(&runtime, wire, &request);
    let response = match send_stream(
        &state.inner.client,
        &url,
        headers,
        &sent,
        &state.inner.config,
    )
    .await
    {
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
        let mut sse = SseState::new(
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
        );
        sse.nudge_text = nudge_text;
        return Ok(streaming_response(sse));
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
            None => {
                let sent = outgoing_body(&runtime, wire, &request);
                send_generation(
                    &state.inner.client,
                    &url,
                    headers,
                    &sent,
                    &state.inner.config,
                )
                .await?
            }
        };
        if !status.is_success() {
            return Ok(response_with(status, content_type.as_deref(), bytes));
        }
        let Some(call) = extract_proxy_call(&bytes) else {
            final_response = Some((status, content_type, bytes));
            break;
        };
        let assistant = response_content(wire, &bytes);
        let (turns, after) = run_proxy_tool(
            state,
            session_id,
            &runtime,
            wire,
            &mut request,
            call,
            &assistant,
            &settings,
            window,
        )?;
        store_proxy_splice(&runtime, &turns, after)?;
    }

    let Some((status, content_type, bytes)) = final_response else {
        return Err(ProxyError::ToolRoundLimit);
    };
    let sent = outgoing_body(&runtime, wire, &request);
    publish_usage(state, session_id, &runtime, &bytes, wire, &sent).await;
    Ok(response_with(status, content_type.as_deref(), bytes))
}

/// The content blocks of a buffered Anthropic response; the other wires keep none.
fn response_content(wire: WireKind, bytes: &Bytes) -> Vec<Value> {
    if wire != WireKind::Anthropic {
        return Vec::new();
    }
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|payload| payload.get("content").and_then(Value::as_array).cloned())
        .unwrap_or_default()
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

    if runtime.reviewer {
        crate::wire::remove_edit_tool(request);
    }
    if settings.proxy_compression && settings.inject_tool {
        inject_tools_for_wire(wire, request, &runtime.pins);
    }
    inject_prompts_for_wire(
        wire,
        request,
        &runtime.pins,
        codebase_memory,
        &runtime.catalog,
    );
    let Some(mut contents) = extract_core_contents(wire, request) else {
        return Ok(PrepareOutcome {
            nudge: None,
            gate: None,
            preflight: None,
            unmatched: None,
        });
    };
    let carried = lock(&runtime.carried)?.clone();
    if !carried.is_empty() {
        contents = carry::join(carried, contents);
    }
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
    // The same miss on every request is noise: report it once until the text changes.
    let unmatched = {
        let mut last = lock(&runtime.last_fold_miss)?;
        if *last == unmatched {
            None
        } else {
            *last = unmatched.clone();
            unmatched
        }
    };
    let folded_call_ids = lock(&runtime.compressor)?.folded_call_ids();
    lock(&runtime.splices)?.apply_skipping(&mut contents, &folded_call_ids)?;
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
    // The stored history is shared with the outgoing turns, which copy only when a nudge rides on
    // them. The nudge goes last, on the final contents, so its turn numbers match what compress sees.
    let history = Arc::new(contents);
    *lock(&runtime.last_history)? = Arc::clone(&history);
    let mut outgoing = Cow::Borrowed(history.as_slice());
    let (nudge, gate) = if settings.proxy_compression && settings.inject_nudge {
        inject_nudge(runtime, &mut outgoing, window, settings)?
    } else {
        (None, None)
    };
    if nudge
        .as_ref()
        .is_some_and(|report| report.kind == NudgeKind::Emergency.name())
    {
        runtime.emergency_truncated.store(true, Ordering::Relaxed);
    }
    write_core_contents(wire, request, &outgoing);
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
        if turns_tokens(contents) <= u64::try_from(window).unwrap_or(u64::MAX) {
            break;
        }
        let end = compressible_end(contents, policy.preserve_recent_messages);
        let Some(run) = foldable_runs(contents, end)
            .into_iter()
            .find(|run| run.len() > 1)
        else {
            break;
        };
        let call = CompressCall {
            start: run.start,
            end: run.end,
            summary: format!(
                "Preflight fold: the request exceeded the context window, so the proxy folded turns {}–{} automatically. The originals are in the session spill dir and `decompress` restores them.",
                run.start,
                run.end - 1
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
            if !marker::is_tool_text(text) {
                continue;
            }
            let kept = text
                .lines()
                .filter(|line| !marker::is_tool_text(line))
                .collect::<Vec<_>>()
                .join("\n");
            part["text"] = json!(kept.trim());
            cleaned = true;
        }
        if cleaned {
            if let Some(raw) = turn.get_mut(turnkeys::RAW) {
                strip_raw_markers(raw);
            }
        }
    }
}

fn strip_raw_markers(raw: &mut Value) {
    if let Some(content) = raw.get("content").and_then(Value::as_str) {
        let kept = content
            .lines()
            .filter(|line| !marker::is_tool_text(line))
            .collect::<Vec<_>>()
            .join("\n");
        raw["content"] = json!(kept.trim());
    }
    if let Some(blocks) = raw.get_mut("content").and_then(Value::as_array_mut) {
        for block in blocks {
            let Some(text) = block.get("text").and_then(Value::as_str) else {
                continue;
            };
            if !marker::is_tool_text(text) {
                continue;
            }
            block["text"] = json!(text
                .lines()
                .filter(|line| { !marker::is_tool_text(line) })
                .collect::<Vec<_>>()
                .join("\n")
                .trim());
        }
    }
}

/// The body as it goes upstream. While an emergency holds, older results are cut short on this
/// copy only: the stored request keeps every result, so a compress or decompress run in it works
/// on the full text.
fn outgoing_body<'a>(
    runtime: &SessionRuntime,
    wire: WireKind,
    request: &'a Value,
) -> Cow<'a, Value> {
    if !runtime.emergency_truncated.load(Ordering::Relaxed) {
        return Cow::Borrowed(request);
    }
    let mut body = request.clone();
    if let Some(mut contents) = extract_core_contents(wire, request_body_mut(&mut body)) {
        truncate_emergency(&mut contents);
        write_core_contents(wire, request_body_mut(&mut body), &contents);
    }
    Cow::Owned(body)
}

/// Removes the nudge turn from a review fork body: the nudge speaks to the main conversation,
/// and the fork must not see it. The wire holds it as a plain user turn, found by its text.
fn drop_nudge(wire: WireKind, body: &mut Value, text: &str) {
    let Some(mut contents) = extract_core_contents(wire, request_body_mut(body)) else {
        return;
    };
    contents.retain(|turn| {
        turn.get("role").and_then(Value::as_str) != Some("user")
            || turn.pointer("/parts/0/text").and_then(Value::as_str) != Some(text)
    });
    write_core_contents(wire, request_body_mut(body), &contents);
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
                        .is_some_and(|text| text.starts_with(marker::TOOL_OK))
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
            if let Some(raw) = turn.get_mut(turnkeys::RAW) {
                crate::wire::set_raw_result_text(raw, &id, &text);
            }
        }
    }
}

#[derive(Debug)]
pub struct NudgeReport {
    pub kind: &'static str,
    pub tokens: u64,
    pub since_last: u64,
    pub text: String,
}

async fn context_window(state: &ProxyState, session_id: &str) -> usize {
    match state.inner.sessions.get(session_id).await {
        Some(meta) => state.inner.models.window(&meta.model).await.unwrap_or(0),
        None => 0,
    }
}

fn kilo(tokens: u64) -> String {
    format!("{:.1}K", tokens as f64 / 1000.0)
}

struct CompressibleView {
    tokens: u64,
    breakdown: String,
    ranges: String,
}

/// The exclusive end of the compressible range `1..end`, shared by the nudge and the preflight
/// backstop. A range never includes the latest user message (`compress` refuses it), and never
/// the preserved tail. Ranges start at turn 1, so a latest message at turn 0 cuts nothing.
fn compressible_end(turns: &[Value], preserve_recent_messages: usize) -> usize {
    let latest = latest_user_turn(turns)
        .filter(|&latest| latest > 0)
        .unwrap_or(usize::MAX);
    turns
        .len()
        .saturating_sub(preserve_recent_messages)
        .min(latest)
}

/// The compressible view: only the foldable runs inside `1..end` count, so protected turns
/// (system reminders) add neither tokens nor a range to the nudge.
fn compressible_view(turns: &[Value], preserve_recent_messages: usize) -> CompressibleView {
    let end = compressible_end(turns, preserve_recent_messages);
    let runs = foldable_runs(turns, end);
    let tokens: u64 = runs
        .iter()
        .map(|run| turns_tokens(&turns[run.clone()]))
        .sum();
    let ranges = if runs.is_empty() {
        "Compressible ranges (1, oldest first): none".to_string()
    } else {
        let mut lines = vec!["Compressible ranges (1, oldest first):".to_string()];
        for run in &runs {
            // The compress arguments verbatim: `end` is exclusive there.
            lines.push(format!(
                "  start {}, end {}  {} turns  {} tokens",
                run.start,
                run.end,
                run.len(),
                kilo(turns_tokens(&turns[run.clone()]))
            ));
        }
        lines.join("\n")
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
    contents: &mut Cow<'_, [Value]>,
    window: usize,
    settings: &Settings,
) -> Result<(Option<NudgeReport>, Option<NudgeState>), ProxyError> {
    let Some(result_index) = last_tool_result_index(contents) else {
        return Ok((None, None));
    };
    let preserve = settings.preserve_recent_messages as usize;
    let view = compressible_view(contents, preserve);
    let tokens = match runtime.last_input_tokens.load(Ordering::Relaxed) {
        0 => turns_tokens(contents),
        used => used,
    };
    let active_blocks = lock(&runtime.compressor)?.status().active_blocks as u64;
    let config = NudgeConfig {
        max_context_limit_pct: u64::from(settings.max_context_limit),
        emergency_threshold_pct: u64::from(settings.emergency_threshold_percent),
        min_pressure_benefit_tokens: u64::from(settings.min_pressure_benefit_tokens),
        tier2_trigger: u64::from(settings.tier2_trigger),
        tier3_trigger: u64::from(settings.tier3_trigger),
        estimate: runtime.last_input_tokens.load(Ordering::Relaxed) == 0,
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
    // The nudge turn is proxy-only: a fold must not store it, and no wire may see it.
    contents
        .to_mut()
        .insert(result_index + 1, turnkeys::nudge_turn(&text));
    let report = NudgeReport {
        kind: fired.kind.name(),
        tokens,
        since_last: fired.since_last,
        text,
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
    // Images ride as inlineData parts in core format; the text part is left out when empty.
    let mut parts: Vec<Value> = entries
        .iter()
        .flat_map(|entry| entry.images.iter())
        .map(|image| json!({ "inlineData": { "mimeType": image.mime_type, "data": image.data_base64 } }))
        .collect();
    let text = entries
        .iter()
        .map(|entry| entry.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !text.is_empty() {
        parts.insert(0, json!({ "text": text }));
    }
    let splice = Splice {
        id: format!("steer-{}", ids.join("-")),
        tool_result_id: turn_id,
        marker: String::new(),
        turns: vec![json!({ "role": "user", "parts": parts })],
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

pub(crate) fn request_body_mut(root: &mut Value) -> &mut Value {
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
    assistant: &[Value],
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
    let turns = proxy_turns(
        wire,
        assistant,
        &[(&call, text.as_str())],
        &turnkeys::splice_marker(&call.id),
    );
    contents.extend(turns.clone());
    write_core_contents(wire, request, &contents);
    Ok((turns, after))
}

/// The turn a proxy pair follows: the newest turn of the request that the harness resends. The
/// transient nudge is skipped, because no later request carries it.
fn splice_anchor(contents: &[Value]) -> Option<String> {
    contents
        .iter()
        .rev()
        .find(|turn| !turnkeys::is_transient(turn))
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
            marker::tool_result(&call.name, &text)
        }
        Err(error) => marker::tool_failure(&call.name, error),
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

fn proxy_result_part(call: &ProxyCall, text: &str, marker: &str) -> Value {
    json!({
        "functionResponse": {
            "name": call.name,
            "id": call.id,
            "response": { "result": text, "splice": marker }
        }
    })
}

fn proxy_pair(call: &ProxyCall, text: &str, marker: &str) -> Vec<Value> {
    vec![
        json!({ "role": "model", "parts": [call.part.clone()] }),
        json!({ "role": "user", "parts": [proxy_result_part(call, text, marker)] }),
    ]
}

/// The turns a round of proxy calls adds to the request. Anthropic gets one model turn holding the
/// response's own blocks and one user turn with every result, as the Messages API requires; the
/// other wires get a model/user pair per call.
fn proxy_turns(
    wire: WireKind,
    assistant: &[Value],
    completed: &[(&ProxyCall, &str)],
    marker: &str,
) -> Vec<Value> {
    if wire != WireKind::Anthropic {
        return completed
            .iter()
            .flat_map(|(call, text)| proxy_pair(call, text, marker))
            .collect();
    }
    let calls: Vec<&ProxyCall> = completed.iter().map(|(call, _)| *call).collect();
    let results: Vec<Value> = completed
        .iter()
        .map(|(call, text)| proxy_result_part(call, text, marker))
        .collect();
    vec![
        anthropic_model_turn(assistant, &calls),
        json!({ "role": "user", "parts": results }),
    ]
}

fn store_proxy_splice(
    runtime: &SessionRuntime,
    turns: &[Value],
    after: Option<String>,
) -> Result<String, ProxyError> {
    let marker = first_part_id(turns, "functionCall")
        .map(|id| turnkeys::splice_marker(&id))
        .unwrap_or_else(|| turnkeys::splice_marker("unknown"));
    let anchor = first_part_id(turns, "functionResponse").unwrap_or_default();
    let splice = Splice::new(&anchor, turns.to_vec())
        .with_marker(marker)
        .with_after_turn(after);
    lock(&runtime.splices)?.insert(splice)?;
    Ok(anchor)
}

/// The id of the first part of `kind` (a call or a result) across the turns.
fn first_part_id(turns: &[Value], kind: &str) -> Option<String> {
    turns
        .iter()
        .filter_map(|turn| turn.get("parts")?.as_array())
        .flatten()
        .find_map(|part| part.get(kind)?.get("id")?.as_str())
        .map(str::to_string)
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
            refuse_block_folds(contents, &calls)?;
            refuse_cache_costly_folds(contents, &calls)?;
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
            let report = lock(&runtime.compressor)
                .map_err(|error| error.to_string())?
                .search_context(&query, limit)?;
            Ok(report.render())
        }
        "acp_status" => {
            let status = lock(&runtime.compressor)
                .map_err(|error| error.to_string())?
                .status();
            let template = runtime.pins.text(ACP_STATUS_REPORT).unwrap_or_default();
            acp_status_text(template, contents, window, &status, settings)
                .map_err(|error| error.to_string())
        }
        other => Err(format!("not a proxy tool: {other}")),
    }
}

fn acp_status_text(
    template: &str,
    contents: &[Value],
    window: usize,
    status: &crate::compressor::CompressorStatus,
    settings: &Settings,
) -> Result<String, PromptError> {
    let view = compressible_view(contents, settings.preserve_recent_messages as usize);
    format_acp_status(
        template,
        turns_tokens(contents),
        window,
        status.active_blocks,
        status.blocks,
        status.spilled_turns,
        status.tokens_compressed,
        status.restored_blocks,
        &view.ranges,
    )
}

/// The `acp_status` report, filled into the Session's pinned `acp_status.report` template.
#[allow(clippy::too_many_arguments)]
fn format_acp_status(
    template: &str,
    tokens: u64,
    window: usize,
    active_blocks: usize,
    total_blocks: usize,
    stored_messages: usize,
    tokens_compressed: u64,
    restored: usize,
    ranges: &str,
) -> Result<String, PromptError> {
    let percent = if window == 0 {
        0
    } else {
        tokens.saturating_mul(100) / u64::try_from(window).unwrap_or(u64::MAX)
    };
    let vars = PromptVars {
        status_tokens: kilo(tokens),
        status_window: kilo(u64::try_from(window).unwrap_or(u64::MAX)),
        status_percent: percent.to_string(),
        status_active: active_blocks.to_string(),
        status_total: total_blocks.to_string(),
        status_stored: stored_messages.to_string(),
        status_compressed: kilo(tokens_compressed),
        status_restored: restored.to_string(),
        status_ranges: ranges.to_string(),
        ..PromptVars::default()
    };
    render_text(ACP_STATUS_REPORT, template, &vars)
}

/// A block never folds again: it stays as written and only the turns added after it fold.
fn refuse_block_folds(contents: &[Value], calls: &[CompressCall]) -> Result<(), String> {
    for call in calls {
        if contents
            .get(call.start..call.end)
            .is_some_and(|turns| turns.iter().any(crate::compressor::is_summary_turn))
        {
            return Err(format!(
                "range {}..{} includes a block summary; blocks never fold again, fold only turns added after them (the nudge lists them)",
                call.start, call.end
            ));
        }
    }
    Ok(())
}

/// A fold rewrites the history from its first turn on, so the provider re-reads everything after
/// it uncached. A small fold early in a big history costs far more than it frees: refuse it.
fn refuse_cache_costly_folds(contents: &[Value], calls: &[CompressCall]) -> Result<(), String> {
    let Some(first) = calls.iter().map(|call| call.start).min() else {
        return Ok(());
    };
    let Some(tail) = contents.get(first..) else {
        return Ok(());
    };
    let after = turns_tokens(tail);
    let freed: u64 = calls
        .iter()
        .filter_map(|call| contents.get(call.start..call.end))
        .map(turns_tokens)
        .sum();
    if after > 20_000 && freed * 4 < after {
        return Err(format!(
            "fold frees about {freed} tokens but rewrites {after}: the provider cache restarts at turn {first}. Fold a bigger or later range (the nudge lists them), or do not fold yet"
        ));
    }
    Ok(())
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
    let (start, end) = (number_arg(args, "start")?, number_arg(args, "end")?);
    let summary = string_arg(args, "summary")?;
    // A tiny summary over a big range loses the work; the model then re-reads.
    if end.saturating_sub(start) > 6 && summary.trim().len() < 300 {
        return Err(format!(
            "summary too short for {} turns: write at least 300 characters with paths, commands, decisions and open items",
            end - start
        ));
    }
    Ok(CompressCall {
        start,
        end,
        summary,
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

/// The wait between two sends of a retried request: product behaviour, zero in tests.
#[allow(clippy::disallowed_methods)]
async fn backoff(wait: Duration) {
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
}

/// Sends one generation request, trying again after a connect failure or a gateway status
/// (502/503/504): nothing has reached the harness yet, so a retry is invisible to it.
async fn send_stream(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
    config: &ProxyConfig,
) -> Result<reqwest::Response, ProxyError> {
    let bytes = serde_json::to_vec(body)?;
    let encoding = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok());
    let bytes = crate::encoding::encode(encoding, bytes)?;
    let mut attempt = 0;
    loop {
        let mut request = client.post(url);
        request = with_headers(request, headers);
        let request = request.header(header::CONTENT_TYPE, "application/json");
        let sent = timeout(config.round_timeout, request.body(bytes.clone()).send())
            .await
            .map_err(|_| ProxyError::Timeout)
            .and_then(|result| result.map_err(ProxyError::Upstream));
        let retryable = match &sent {
            Ok(response) => matches!(response.status().as_u16(), 502..=504),
            Err(ProxyError::Upstream(error)) => error.is_connect() || error.is_request(),
            Err(_) => false,
        };
        if !retryable || attempt >= SEND_RETRIES {
            return sent;
        }
        backoff(config.retry_backoff * 2u32.pow(attempt)).await;
        attempt += 1;
    }
}

/// The background skill review: the fork conversation runs to its end or `MAX_ROUNDS`, its tool
/// calls go through the daemon's executor, and nothing of it reaches the Session.
async fn run_review(
    proxy: ProxyState,
    runtime: Arc<SessionRuntime>,
    session_id: String,
    wire: WireKind,
    (url, headers): (String, HeaderMap),
    mut body: Value,
    tools: Arc<dyn ReviewTools>,
) {
    let mut changes = 0usize;
    let mut skill_args: Vec<Value> = Vec::new();
    for round in 1..=review::MAX_ROUNDS {
        let sent = send_generation(
            &proxy.inner.client,
            &url,
            &headers,
            &body,
            &proxy.inner.config,
        )
        .await;
        let bytes = match sent {
            Ok((status, _, bytes)) if status.is_success() => bytes,
            Ok((status, _, bytes)) => {
                let text = String::from_utf8_lossy(&bytes);
                let text: String = text.chars().take(300).collect();
                log_review(&runtime, round, None, 0, Some(format!("{status}: {text}")));
                break;
            }
            Err(error) => {
                log_review(&runtime, round, None, 0, Some(error.to_string()));
                break;
            }
        };
        let Ok(response) = serde_json::from_slice::<Value>(&bytes) else {
            log_review(
                &runtime,
                round,
                None,
                0,
                Some("response is not JSON".into()),
            );
            break;
        };
        let calls = review::calls(wire, &response);
        log_review(
            &runtime,
            round,
            meter::extract_usage(&bytes),
            calls.len(),
            None,
        );
        if calls.is_empty() {
            break;
        }
        let mut results = Vec::new();
        for call in calls {
            let result = tools.call(&session_id, &call.name, &call.args).await;
            if result.is_ok() && call.name.ends_with("skill_manage") {
                changes += 1;
                skill_args.extend(call.args["ops"].as_array().cloned().unwrap_or_default());
            }
            results.push((call, result.unwrap_or_else(|error| error)));
        }
        review::append_round(wire, &mut body, &response, &results);
    }
    if changes > 0 {
        proxy.inner.events.publish(SessionEvent::new(
            &session_id,
            0,
            EventKind::Compression,
            json!({ "action": "skill-review", "text": format!("skill review: {changes} skill change(s)"), "ops": skill_args }),
        ));
    }
    if let Ok(mut state) = lock(&runtime.review) {
        state.finished();
    }
}

/// Appends one line per review round to the Session's `review.jsonl`. Best effort: a log that
/// cannot be written never stops the review.
fn log_review(
    runtime: &SessionRuntime,
    round: usize,
    usage: Option<meter::Usage>,
    calls: usize,
    error: Option<String>,
) {
    use std::io::Write;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let usage = usage.unwrap_or_default();
    let line = json!({
        "at": at,
        "round": round,
        "input": usage.input_tokens,
        "cached": usage.cached_tokens,
        "created": usage.cache_creation_tokens,
        "output": usage.output_tokens,
        "calls": calls,
        "error": error,
    });
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&runtime.review_log)
    {
        let _ = writeln!(file, "{line}");
    }
}

async fn send_generation(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
    config: &ProxyConfig,
) -> Result<(StatusCode, Option<String>, Bytes), ProxyError> {
    let round_timeout = config.round_timeout;
    let response = send_stream(client, url, headers, body, config).await?;
    let status = axum_status(response.status());
    let content_type = header_value(response.headers(), header::CONTENT_TYPE.as_str());
    let bytes = timeout(round_timeout, response.bytes())
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
    let window = state.context_window(session_id).await;
    let mut data = json!({
        "inputTokens": usage.input_tokens,
        "outputTokens": usage.output_tokens,
        "cachedTokens": usage.cached_tokens,
        "cacheCreationTokens": usage.cache_creation_tokens,
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
        ProxyError::Upstream(_) | ProxyError::UpstreamStatus(_) => StatusCode::BAD_GATEWAY,
    };
    response_with(
        status,
        Some("text/plain; charset=utf-8"),
        Bytes::from(error_text(error)),
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
    /// A Google reading waiting for the stream end; see `capture_usage`.
    held: Option<meter::Usage>,
    /// The usage last published for this stream, so a chunk and its events do not publish twice.
    published: Option<meter::Usage>,
    /// The signature of the step being streamed: the model signs its first call.
    step_signature: Option<Value>,
    /// Tool calls a pi wire streams in fragments; the wire decides run or hide.
    stream_calls: Vec<StreamCall>,
    stream_events: Vec<Bytes>,
    stream_finish: Vec<Bytes>,
    proxy_round: bool,
    /// The response's content blocks as streamed, kept for an Anthropic follow-up.
    anthropic: AnthropicBlocks,
    /// Names of the tool calls this round handed to the harness; the review counts rounds by them.
    forwarded: Vec<String>,
    /// The nudge text this request carried, so the review fork can leave it out.
    nudge_text: Option<String>,
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
            held: None,
            step_signature: None,
            stream_calls: Vec::new(),
            stream_events: Vec::new(),
            stream_finish: Vec::new(),
            proxy_round: false,
            anthropic: AnthropicBlocks::default(),
            forwarded: Vec::new(),
            nudge_text: None,
            settings,
            window,
        }
    }

    /// Publish usage as soon as its bytes arrive: a harness may stop reading the stream
    /// right after the answer, and then a publish deferred to the stream end never runs.
    /// Google streams repeat usage per chunk and an early reading lacks the cache fields, so
    /// there the newest reading is held and published once at the stream end.
    async fn capture_usage(&mut self, bytes: &[u8]) {
        let Some(usage) = meter::extract_usage(bytes) else {
            return;
        };
        if self.published.is_some_and(|last| last.same_context(&usage)) {
            return;
        }
        self.published = Some(usage);
        self.runtime
            .last_input_tokens
            .store(usage.input_tokens, Ordering::Relaxed);
        self.held = Some(usage);
        if self.wire != WireKind::Google {
            self.flush_usage().await;
        }
    }

    async fn flush_usage(&mut self) {
        let Some(usage) = self.held.take() else {
            return;
        };
        publish_usage_event(
            &self.proxy,
            &self.session_id,
            usage,
            crate::context::ContextBreakdown::from_request(
                self.wire,
                &outgoing_body(&self.runtime, self.wire, &self.request),
            ),
        )
        .await;
    }

    async fn follow_up(&mut self) -> Result<(), ProxyError> {
        let sent = outgoing_body(&self.runtime, self.wire, &self.request);
        let response = send_stream(
            &self.proxy.inner.client,
            &self.url,
            &self.headers,
            &sent,
            &self.proxy.inner.config,
        )
        .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let body: String = body.chars().take(500).collect();
            return Err(ProxyError::UpstreamStatus(format!("{status}: {body}")));
        }
        self.stream = Some(response.bytes_stream().boxed());
        self.published = None;
        self.forwarded.clear();
        self.step_signature = None;
        self.proxy_round = false;
        self.anthropic = AnthropicBlocks::default();
        self.stream_calls.clear();
        self.rounds += 1;
        Ok(())
    }

    /// A round ended without a proxy call: count it, and start the background skill review when due.
    fn review_check(&mut self) {
        let Some(tools) = self.proxy.inner.config.review_tools.clone() else {
            return;
        };
        if !review::offers_skill_tool(&self.request) {
            return;
        }
        let interval = self.settings.skills_review_interval;
        let due = lock(&self.runtime.review)
            .is_ok_and(|mut state| state.round_ended(&self.forwarded, interval));
        if !due {
            return;
        }
        let prompt = self
            .proxy
            .inner
            .prompts
            .render(prompts::REVIEW_SKILLS, &PromptVars::default());
        let Ok(prompt) = prompt else {
            if let Ok(mut state) = lock(&self.runtime.review) {
                state.finished();
            }
            return;
        };
        let mut sent = outgoing_body(&self.runtime, self.wire, &self.request).into_owned();
        if let Some(text) = &self.nudge_text {
            drop_nudge(self.wire, &mut sent, text);
        }
        let (url, body) = review::fork_request(self.wire, &self.url, &sent, &prompt);
        // The headers live only as long as the review task; they are never stored.
        tokio::spawn(run_review(
            self.proxy.clone(),
            Arc::clone(&self.runtime),
            self.session_id.clone(),
            self.wire,
            (url, self.headers.clone()),
            body,
            tools,
        ));
    }

    fn finish_round(&mut self) -> Result<(), ProxyError> {
        if self.completed.is_empty() {
            self.done = true;
            self.review_check();
            return Ok(());
        }
        let first_id = self.completed[0].call.id.clone();
        let marker = turnkeys::splice_marker(&first_id);
        let calls: Vec<(&ProxyCall, &str)> = self
            .completed
            .iter()
            .map(|completed| (&completed.call, completed.text.as_str()))
            .collect();
        let turns = proxy_turns(self.wire, &self.anthropic.content(), &calls, &marker);

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
            write_core_contents(self.wire, req_body, &contents);
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
        if self.wire == WireKind::Anthropic {
            self.anthropic.observe(&payload);
        }
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
            self.forwarded
                .extend(self.stream_calls.iter().map(|call| call.name.clone()));
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
        write_core_contents(self.wire, req_body, &contents);
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
    let mut harness_names: Vec<String> = Vec::new();
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
                    swallowed_signature.or_else(|| part.get(turnkeys::THOUGHT_SIGNATURE).cloned());
                calls.push(call);
                changed = true;
                continue;
            }
            if let Some(name) = part.pointer("/functionCall/name").and_then(Value::as_str) {
                harness_names.push(name.to_string());
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
    state.forwarded.extend(harness_names);
    for call in calls {
        state.record_call(call);
    }
    Ok(RewriteOutcome { changed, empty })
}

/// A signed part sets the step's signature; a swallowed call without one
/// takes it, because the store replays that call as a turn of its own.
fn remember_or_apply_step_signature(part: &mut Value, step_signature: &mut Option<Value>) {
    if let Some(signature) = part.get(turnkeys::THOUGHT_SIGNATURE) {
        *step_signature = Some(signature.clone());
        return;
    }
    if proxy_call_from_part(part).is_some() {
        if let Some(signature) = step_signature.clone() {
            part[turnkeys::THOUGHT_SIGNATURE] = signature;
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
        if part.get(turnkeys::THOUGHT_SIGNATURE).is_none() {
            part[turnkeys::THOUGHT_SIGNATURE] = signature;
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
            Some(stream) => timeout(state.proxy.inner.config.round_timeout, stream.next()).await,
            None => Ok(None),
        };
        match polled {
            Ok(Some(Ok(chunk))) => {
                state.capture_usage(&chunk).await;
                state.pending.extend_from_slice(&chunk);
            }
            Ok(Some(Err(error))) => {
                state.done = true;
                let error = ProxyError::Upstream(error);
                publish_error(
                    &state.proxy.inner.events,
                    &state.session_id,
                    format!("{error}"),
                );
                return Some((Ok(error_event(&error)), state));
            }
            Ok(None) => {
                state.stream = None;
                if !state.pending.is_empty() {
                    state.pending.extend_from_slice(b"\n\n");
                    continue;
                }
                state.flush_usage().await;
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

/// Decode and body failures say "stream was interrupted" so harness retry policies retry them.
fn error_text(error: &ProxyError) -> String {
    match error {
        ProxyError::Upstream(err) if err.is_decode() || err.is_body() => {
            format!("stream was interrupted: {error}")
        }
        _ => error.to_string(),
    }
}

fn error_event(error: &ProxyError) -> Bytes {
    Bytes::from(sse_error(&error_text(error)))
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
