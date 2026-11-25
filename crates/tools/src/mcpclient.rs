//! MCP client: one server per entry in `<config root>/mcp.json`, either a stdio
//! child process or a Streamable HTTP endpoint. Its tools reach the model only
//! through `execute`; the harness tool list is fixed, so a server is never
//! visible as a tool of its own.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderName, HeaderValue, AUTHORIZATION};
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::ServiceExt;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

use crate::figcraft;
use crate::graph;
use crate::{ToolError, ToolResult};

mod design;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// A url server that failed to connect is left alone this long, so a model that
/// keeps asking for its tools does not handshake on every request.
const FAILURE_BACKOFF: Duration = Duration::from_secs(30);

fn yes() -> bool {
    true
}

/// How an entry reaches its server.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Transport {
    #[default]
    Http,
}

/// One `mcpServers` entry: a command or a url, never both.
#[derive(Clone, Debug, Deserialize)]
struct Entry {
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Sent with every HTTP request; a bearer token belongs here too.
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// Tool names to expose; empty means every tool the server lists.
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default = "yes")]
    enabled: bool,
    /// Shown by `tools.servers()`.
    #[serde(default)]
    summary: String,
    /// Overrides the resolved `project` argument for the `graph` server.
    #[serde(default)]
    project: Option<String>,
    /// Argv tails run before the server first starts, so a binary that has to be
    /// configured (codebase-memory-mcp) is configured the same way every time.
    #[serde(default)]
    setup: Vec<Vec<String>>,
    #[serde(default)]
    transport: Transport,
    /// Hidden from a harness Session unless that Session's Project lists the tool.
    #[serde(default)]
    gated: bool,
    #[serde(default)]
    url: Option<String>,
}

struct Server {
    name: String,
    entry: Entry,
    /// Why this server cannot be used; it still answers with the reason.
    error: Option<String>,
    /// When this url server last failed to connect and why.
    failed: std::sync::Mutex<Option<(Instant, String)>>,
}

impl Server {
    fn recent_failure(&self) -> Option<String> {
        self.entry.url.as_ref()?;
        let failed = self
            .failed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        failed
            .as_ref()
            .and_then(|(at, error)| (*at + FAILURE_BACKOFF > Instant::now()).then(|| error.clone()))
    }

    fn remember_failure(&self, error: &str) {
        if self.entry.url.is_none() {
            return;
        }
        *self
            .failed
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some((Instant::now(), error.to_string()));
    }

    fn clear_failure(&self) {
        *self
            .failed
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }
}

struct Connected {
    service: Arc<RunningService<RoleClient, ()>>,
    tools: Vec<Tool>,
    /// The server's process group: its leader is the direct child and its helpers share it.
    group: Option<u32>,
}

impl Drop for Connected {
    fn drop(&mut self) {
        // A dropped server takes everything it started with it; rmcp only kills the leader.
        if let Some(group) = self.group {
            crate::bash::kill_group(group);
        }
    }
}

/// One MCP tool as `execute` sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpTool {
    pub server: String,
    pub summary: String,
    pub name: String,
    pub call_name: String,
    pub signature: String,
    pub description: String,
}

/// Why `call_within` gave up; `find` maps these onto fallback telemetry.
#[derive(Debug)]
pub enum WithinError {
    /// The cold connect outlived its budget; the tracked task keeps warming.
    Warming,
    /// The RPC outlived its budget after the server connected.
    Timeout,
    /// The server could not connect or the RPC failed.
    Failed(ToolError),
}

pub struct McpRegistry {
    root: PathBuf,
    servers: Vec<Arc<Server>>,
    /// A config that would not load; it costs the MCP tools and nothing else.
    error: Option<String>,
    connected: Arc<RwLock<BTreeMap<String, Arc<Connected>>>>,
    /// Resolved `project` per server, so `list_projects` runs once.
    projects: RwLock<BTreeMap<String, String>>,
    /// One connect lock per server, so concurrent callers spawn one child, not two.
    locks: BTreeMap<String, Arc<Mutex<()>>>,
    /// Connect tasks still running; aborted when the registry drops.
    pending: std::sync::Mutex<Vec<tokio::task::AbortHandle>>,
}

impl McpRegistry {
    pub fn load(root: &Path, file: Option<&Path>, project_tools: Option<&[String]>) -> Self {
        Self::load_with(root, file, &graph::default_bin_dirs(), project_tools)
    }

    /// `bin_dirs` is searched for a native codebase-memory-mcp that replaces the npx fallback.
    /// `project_tools` is the Session Project's enabled tool names; `None` disables gating.
    pub fn load_with(
        root: &Path,
        file: Option<&Path>,
        bin_dirs: &[PathBuf],
        project_tools: Option<&[String]>,
    ) -> Self {
        let mut registry = Self {
            root: root.to_path_buf(),
            servers: Vec::new(),
            error: None,
            connected: Arc::new(RwLock::new(BTreeMap::new())),
            projects: RwLock::new(BTreeMap::new()),
            locks: BTreeMap::new(),
            pending: std::sync::Mutex::new(Vec::new()),
        };
        if let Some(file) = file {
            if let Err(error) = registry.read(file, bin_dirs, project_tools) {
                registry.error = Some(error);
            }
        } else if let Ok(entry) = serde_json::from_value::<Entry>(figcraft::default_entry()) {
            registry.servers.push(Arc::new(Server {
                name: figcraft::SERVER.to_string(),
                entry,
                error: None,
                failed: std::sync::Mutex::new(None),
            }));
        }
        registry.locks = registry
            .servers
            .iter()
            .map(|server| (server.name.clone(), Arc::new(Mutex::new(()))))
            .collect();
        registry
    }

    fn read(
        &mut self,
        file: &Path,
        bin_dirs: &[PathBuf],
        project_tools: Option<&[String]>,
    ) -> Result<(), String> {
        let raw = std::fs::read_to_string(file)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        let parsed: Value =
            serde_json::from_str(&raw).map_err(|error| format!("{}: {error}", file.display()))?;
        let servers = parsed
            .get("mcpServers")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("{}: no mcpServers object", file.display()))?;
        for (name, value) in servers {
            let mut entry: Entry = serde_json::from_value(value.clone())
                .map_err(|error| format!("{}: server {name:?}: {error}", file.display()))?;
            if name == graph::SERVER
                && graph::is_npx_fallback(entry.command.as_deref(), &entry.args)
            {
                if let Some(native) = graph::native_binary(bin_dirs) {
                    entry.command = Some(native.to_string_lossy().into_owned());
                    entry.args.clear();
                }
            }
            if !entry.enabled {
                continue;
            }
            if entry.gated && project_tools.is_some_and(|names| !names.iter().any(|n| n == name)) {
                continue;
            }
            let error = entry_error(name, &entry);
            self.servers.push(Arc::new(Server {
                name: name.clone(),
                entry,
                error,
                failed: std::sync::Mutex::new(None),
            }));
        }
        self.servers
            .sort_by(|left, right| left.name.cmp(&right.name));
        Ok(())
    }

    fn loaded(&self) -> ToolResult<()> {
        match &self.error {
            Some(error) => Err(ToolError::msg(error.clone())),
            None => Ok(()),
        }
    }

    /// Every exposed tool of every reachable server, connecting on first use.
    pub async fn tools(&self) -> ToolResult<Vec<McpTool>> {
        self.loaded()?;
        let mut ready = self.empty();
        self.drain(self.spawn_connects(), &mut ready).await;
        Ok(self.collect(ready))
    }

    /// Like `tools`, but gives up after `deadline`; slow servers keep connecting.
    pub async fn tools_within(&self, deadline: Duration) -> Vec<McpTool> {
        if self.loaded().is_err() {
            return Vec::new();
        }
        let mut ready = self.empty();
        let drain = self.drain(self.spawn_connects(), &mut ready);
        let _ = tokio::time::timeout(deadline, drain).await;
        self.collect(ready)
    }

    fn empty(&self) -> Vec<Option<Arc<Connected>>> {
        (0..self.servers.len()).map(|_| None).collect()
    }

    /// One task per server, each answering `(server index, connection)`.
    fn spawn_connects(&self) -> mpsc::Receiver<(usize, ToolResult<Arc<Connected>>)> {
        let (tx, rx) = mpsc::channel(self.servers.len().max(1));
        for (idx, server) in self.servers.iter().enumerate() {
            if server.error.is_some() {
                continue;
            }
            let server = Arc::clone(server);
            let Some(lock) = self.locks.get(&server.name).cloned() else {
                continue;
            };
            let root = self.root.clone();
            let connected = Arc::clone(&self.connected);
            let tx = tx.clone();
            let task = tokio::spawn(async move {
                let result = connect_server(root, server, connected, lock).await;
                let _ = tx.send((idx, result)).await;
            });
            self.track(task.abort_handle());
        }
        rx
    }

    /// Keeps the abort handles of unfinished connects; `Drop` cancels them.
    fn track(&self, handle: tokio::task::AbortHandle) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|handle| !handle.is_finished());
        pending.push(handle);
    }

    async fn drain(
        &self,
        mut rx: mpsc::Receiver<(usize, ToolResult<Arc<Connected>>)>,
        ready: &mut [Option<Arc<Connected>>],
    ) {
        while let Some((idx, result)) = rx.recv().await {
            match result {
                Ok(connected) => ready[idx] = Some(connected),
                Err(error) => eprintln!("{error}"),
            }
        }
    }

    /// Tools in `self.servers` order, which is the config's sorted server order.
    fn collect(&self, ready: Vec<Option<Arc<Connected>>>) -> Vec<McpTool> {
        let mut tools = Vec::new();
        for (server, connected) in self.servers.iter().zip(ready) {
            if let Some(connected) = connected {
                tools.extend(server_tools(server, &connected.tools));
            }
        }
        tools
    }

    /// Calls `server.tool`; a server's own error text becomes our error.
    pub async fn call(&self, name: &str, args: &Value) -> ToolResult<String> {
        let server = name.split_once('.').map_or(name, |(server, _)| server);
        match self.attempt(name, args).await {
            // A killed stdio child never restarts by itself. Start it once more for the next call.
            Err(error) if self.transport_closed(server, &error).await => {
                self.connected.write().await.remove(server);
                self.attempt(name, args).await
            }
            other => other,
        }
    }

    /// Whether `name` is a configured server that has not failed to load.
    pub fn has_server(&self, name: &str) -> bool {
        self.servers
            .iter()
            .any(|server| server.name == name && server.error.is_none())
    }

    /// Calls `name` with a warmup budget for a cold connect and a tighter budget for the
    /// RPC. A timed-out connect is not aborted: it stays tracked and the next call finds
    /// it warm.
    pub async fn call_within(
        &self,
        name: &str,
        args: &Value,
        connect_budget: Duration,
        call_budget: Duration,
    ) -> Result<String, WithinError> {
        self.loaded().map_err(WithinError::Failed)?;
        let Some((server_name, _)) = name.split_once('.') else {
            return Err(WithinError::Failed(ToolError::msg(format!(
                "unknown tool {name:?}"
            ))));
        };
        let Some(server) = self.servers.iter().find(|it| it.name == server_name) else {
            return Err(WithinError::Failed(ToolError::msg(format!(
                "unknown tool {name:?}"
            ))));
        };
        if let Some(error) = &server.error {
            return Err(WithinError::Failed(ToolError::msg(error.clone())));
        }
        if !self.connected.read().await.contains_key(server_name) {
            let warm = self.spawn_warm(Arc::clone(server));
            match tokio::time::timeout(connect_budget, warm).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(error))) => return Err(WithinError::Failed(error)),
                Ok(Err(_)) => {
                    return Err(WithinError::Failed(ToolError::msg(
                        "the connect task ended without an answer",
                    )))
                }
                Err(_) => return Err(WithinError::Warming),
            }
        }
        match tokio::time::timeout(call_budget, self.attempt(name, args)).await {
            Ok(Ok(text)) => Ok(text),
            Ok(Err(error)) => Err(WithinError::Failed(error)),
            Err(_) => Err(WithinError::Timeout),
        }
    }

    /// A cold connect in its own tracked task, so a caller's budget expiring leaves the
    /// handshake running instead of aborting it mid-flight.
    fn spawn_warm(&self, server: Arc<Server>) -> oneshot::Receiver<ToolResult<()>> {
        let (tx, rx) = oneshot::channel();
        let root = self.root.clone();
        let connected = Arc::clone(&self.connected);
        let lock = self.locks.get(&server.name).cloned();
        let task = tokio::spawn(async move {
            let result = match lock {
                Some(lock) => connect_server(root, server, connected, lock)
                    .await
                    .map(|_| ()),
                None => Err(ToolError::msg("unknown mcp server")),
            };
            let _ = tx.send(result);
        });
        self.track(task.abort_handle());
        rx
    }

    async fn transport_closed(&self, server: &str, error: &ToolError) -> bool {
        error.to_string().contains("Transport closed")
            || self
                .connected
                .read()
                .await
                .get(server)
                .is_some_and(|ready| ready.service.is_closed())
    }

    async fn attempt(&self, name: &str, args: &Value) -> ToolResult<String> {
        self.loaded()?;
        let Some((server_name, tool)) = name.split_once('.') else {
            return Err(ToolError::msg(format!("unknown tool {name:?}")));
        };
        let Some(server) = self.servers.iter().find(|it| it.name == server_name) else {
            return Err(ToolError::msg(format!("unknown tool {name:?}")));
        };
        if let Some(error) = &server.error {
            return Err(ToolError::msg(error.clone()));
        }
        let connected = self.ensure(server_name).await?;
        let args = self.arguments(server, tool, args).await?;
        if server_name == figcraft::SERVER {
            design::resync(&self.root, server, &connected).await?;
        }
        let result = raw_call(server, &connected, tool, args).await;
        if server_name == figcraft::SERVER {
            design::sync(server, &connected).await?;
        }
        result
    }

    async fn arguments(&self, server: &Server, tool: &str, args: &Value) -> ToolResult<Value> {
        let mut args = args.clone();
        if server.name == figcraft::SERVER {
            let _ = figcraft::ensure_design_file(&self.root);
            if let Value::Object(map) = &mut args {
                if matches!(tool, "open_file" | "save_file") && !map.contains_key("path") {
                    let path = figcraft::design_path(&self.root);
                    map.insert(
                        "path".to_string(),
                        Value::String(path.to_string_lossy().into_owned()),
                    );
                }
            }
        }
        if let Some(project) = self.project(server, tool).await {
            if let Value::Object(map) = &mut args {
                map.entry("project").or_insert(Value::String(project));
            }
        }
        Ok(args)
    }

    async fn project(&self, server: &Server, tool: &str) -> Option<String> {
        if server.name != graph::SERVER || matches!(tool, "list_projects" | "index_status") {
            return None;
        }
        if let Some(project) = &server.entry.project {
            return Some(project.clone());
        }
        if let Some(cached) = self.projects.read().await.get(&server.name) {
            return Some(cached.clone());
        }
        let resolved = self.resolve_project(server).await;
        self.projects
            .write()
            .await
            .insert(server.name.clone(), resolved.clone());
        Some(resolved)
    }

    /// `list_projects` knows the name when the root is indexed; otherwise the
    /// name codebase-memory derives from the path is the one auto-index creates.
    async fn resolve_project(&self, server: &Server) -> String {
        let fallback = graph::project_from_path(&self.root);
        let Ok(connected) = self.ensure(&server.name).await else {
            return fallback;
        };
        let args = serde_json::json!({"format": "json", "limit": 500});
        let resolved = raw_call(server, &connected, "list_projects", args)
            .await
            .ok()
            .and_then(|output| graph::match_project(&output, &self.root));
        resolved.unwrap_or(fallback)
    }

    async fn ensure(&self, name: &str) -> ToolResult<Arc<Connected>> {
        let server = self
            .servers
            .iter()
            .find(|it| it.name == name)
            .ok_or_else(|| ToolError::msg(format!("unknown tool {name:?}")))?;
        if let Some(error) = &server.error {
            return Err(ToolError::msg(error.clone()));
        }
        let lock = self
            .locks
            .get(name)
            .cloned()
            .ok_or_else(|| ToolError::msg(format!("unknown tool {name:?}")))?;
        connect_server(
            self.root.clone(),
            Arc::clone(server),
            Arc::clone(&self.connected),
            lock,
        )
        .await
    }
}

impl Drop for McpRegistry {
    fn drop(&mut self) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        for handle in pending.drain(..) {
            handle.abort();
        }
    }
}

/// A ready connection, or a new one held under this server's connect lock. The lock is
/// per server so a slow server never delays another.
async fn connect_server(
    root: PathBuf,
    server: Arc<Server>,
    connected: Arc<RwLock<BTreeMap<String, Arc<Connected>>>>,
    lock: Arc<Mutex<()>>,
) -> ToolResult<Arc<Connected>> {
    if let Some(ready) = ready(&connected, &server.name).await {
        return Ok(ready);
    }
    let _guard = lock.lock().await;
    if let Some(ready) = ready(&connected, &server.name).await {
        return Ok(ready);
    }
    if let Some(error) = server.recent_failure() {
        return Err(ToolError::msg(error));
    }
    let ready = match connect(&root, &server).await {
        Ok(ready) => {
            server.clear_failure();
            Arc::new(ready)
        }
        Err(error) => {
            server.remember_failure(&error.to_string());
            return Err(error);
        }
    };
    if server.name == figcraft::SERVER {
        design::connect_design(&root, &server, &ready).await?;
    }
    connected
        .write()
        .await
        .insert(server.name.clone(), Arc::clone(&ready));
    Ok(ready)
}

/// The cached connection unless the server died; a dead one is dropped so it restarts.
async fn ready(
    connected: &RwLock<BTreeMap<String, Arc<Connected>>>,
    name: &str,
) -> Option<Arc<Connected>> {
    let cached = connected.read().await.get(name).cloned();
    match cached {
        Some(cached) if !cached.service.is_closed() => Some(cached),
        Some(_) => {
            // Re-check under the write lock: another task may have reconnected already.
            let mut map = connected.write().await;
            if map
                .get(name)
                .is_some_and(|cached| cached.service.is_closed())
            {
                map.remove(name);
            }
            None
        }
        None => None,
    }
}

fn exposes(entry: &Entry, tool: &str) -> bool {
    (entry.include.is_empty() || entry.include.iter().any(|it| it == tool))
        && !entry.exclude.iter().any(|it| it == tool)
}

/// The marker that records a finished setup. It lives in the server's cache dir, because
/// codebase-memory keeps its configuration there, and holds the setup it applied.
const SETUP_MARKER: &str = "emberloop-setup.json";

fn setup_marker(server: &Server) -> Option<PathBuf> {
    let cache = server.entry.env.get("CBM_CACHE_DIR")?;
    Some(PathBuf::from(cache).join(SETUP_MARKER))
}

/// Best effort: a setup step that fails costs the log line, not the server. A setup that
/// finished once for this cache dir is not repeated by later Sessions; a failed one reruns.
async fn run_setup(root: &Path, server: &Server) {
    if server.entry.setup.is_empty() {
        return;
    }
    let record = serde_json::to_string(&server.entry.setup).unwrap_or_default();
    let marker = setup_marker(server);
    if let Some(marker) = &marker {
        if fs::read_to_string(marker).ok().as_deref() == Some(record.as_str()) {
            return;
        }
    }
    let mut finished = true;
    for extra in &server.entry.setup {
        let mut command =
            tokio::process::Command::new(server.entry.command.clone().unwrap_or_default());
        command
            .args(&server.entry.args)
            .args(extra)
            .envs(&server.entry.env)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match tokio::time::timeout(CONNECT_TIMEOUT, command.status()).await {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => {
                finished = false;
                eprintln!(
                    "mcp server {:?}: setup {extra:?} gave {status}",
                    server.name
                );
            }
            Ok(Err(error)) => {
                finished = false;
                eprintln!("mcp server {:?}: setup {extra:?}: {error}", server.name);
            }
            Err(_) => {
                finished = false;
                eprintln!("mcp server {:?}: setup {extra:?} timed out", server.name);
            }
        }
    }
    if let (true, Some(marker)) = (finished, marker) {
        if let Some(dir) = marker.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(marker, record);
    }
}

async fn connect(root: &Path, server: &Server) -> ToolResult<Connected> {
    match (&server.entry.url, server.entry.transport) {
        (Some(url), Transport::Http) => connect_http(server, url).await,
        (None, _) => connect_stdio(root, server).await,
    }
}

/// Why `entry` cannot be used, or `None`. No header value ever reaches the text.
fn entry_error(name: &str, entry: &Entry) -> Option<String> {
    let command = entry
        .command
        .as_deref()
        .map(str::trim)
        .filter(|command| !command.is_empty());
    let refuse = |why: &str| Some(format!("mcp server {name:?}: {why}"));
    match (command.is_some(), entry.url.is_some()) {
        (true, false) => None,
        (true, true) => refuse("give a command or a url, not both"),
        (false, true) => {
            if !entry.args.is_empty() || !entry.setup.is_empty() {
                return refuse("url entries cannot have args or setup");
            }
            let url = entry.url.as_deref().unwrap_or_default();
            let valid = reqwest::Url::parse(url)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some());
            if valid {
                None
            } else {
                refuse("url must be an http(s) url such as https://host/mcp")
            }
        }
        (false, false) => refuse("give a command or a url"),
    }
}

async fn connect_stdio(root: &Path, server: &Server) -> ToolResult<Connected> {
    run_setup(root, server).await;
    let mut command =
        tokio::process::Command::new(server.entry.command.clone().unwrap_or_default());
    command
        .args(&server.entry.args)
        .envs(&server.entry.env)
        // codebase-memory auto-indexes its cwd; the Project root, not the Session work dir.
        .current_dir(root);
    #[cfg(unix)]
    command.process_group(0);
    let transport = TokioChildProcess::new(command)
        .map_err(|error| ToolError::msg(format!("mcp server {:?}: {error}", server.name)))?;
    let group = transport.id();
    let service = tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport))
        .await
        .map_err(|_| {
            ToolError::msg(format!(
                "mcp server {:?} did not start within {}s",
                server.name,
                CONNECT_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|error| ToolError::msg(format!("mcp server {:?}: {error}", server.name)))?;
    connected(server, Arc::new(service), group).await
}

/// List the tools a freshly connected service exposes.
async fn connected(
    server: &Server,
    service: Arc<RunningService<RoleClient, ()>>,
    group: Option<u32>,
) -> ToolResult<Connected> {
    let listed = tokio::time::timeout(CONNECT_TIMEOUT, service.list_all_tools())
        .await
        .map_err(|_| {
            ToolError::msg(format!(
                "mcp server {:?} did not list its tools within {}s",
                server.name,
                CONNECT_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|error| ToolError::msg(format!("mcp server {:?}: {error}", server.name)))?;
    let tools = listed
        .into_iter()
        .filter(|tool| exposes(&server.entry, &tool.name))
        .collect();
    Ok(Connected {
        service,
        tools,
        group,
    })
}

async fn raw_call(
    server: &Server,
    connected: &Connected,
    tool: &str,
    args: Value,
) -> ToolResult<String> {
    let arguments = match args {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    let params = CallToolRequestParams::new(tool.to_string()).with_arguments(arguments);
    let call = connected.service.call_tool(params);
    let result = tokio::time::timeout(CALL_TIMEOUT, call)
        .await
        .map_err(|_| {
            ToolError::msg(format!(
                "mcp tool {tool:?} did not answer within {}s",
                CALL_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|error| ToolError::msg(format!("mcp server {:?}: {error}", server.name)))?;
    render(result)
}

/// Text blocks joined; a structured answer stands in when there is no text.
fn render(result: CallToolResult) -> ToolResult<String> {
    let mut text = String::new();
    for block in &result.content {
        let Some(block) = block.as_text() else {
            continue;
        };
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&block.text);
    }
    if text.is_empty() {
        if let Some(structured) = &result.structured_content {
            text = structured.to_string();
        }
    }
    if result.is_error.unwrap_or(false) {
        let error = if text.is_empty() {
            "the tool reported an error".to_string()
        } else {
            text
        };
        return Err(ToolError::msg(error));
    }
    Ok(text)
}

fn server_tools(server: &Server, tools: &[Tool]) -> Vec<McpTool> {
    tools
        .iter()
        .map(|tool| {
            let call_name = format!("{}.{}", server.name, tool.name);
            McpTool {
                server: server.name.clone(),
                summary: server.entry.summary.clone(),
                name: tool.name.to_string(),
                signature: signature(&call_name, tool),
                call_name,
                description: tool.description.as_deref().unwrap_or_default().to_string(),
            }
        })
        .collect()
}
fn signature(call_name: &str, tool: &Tool) -> String {
    let schema = &tool.input_schema;
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut optional: Vec<&str> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| {
            props
                .keys()
                .filter(|key| !required.contains(&key.as_str()))
                .map(String::as_str)
                .collect()
        })
        .unwrap_or_default();
    optional.sort_unstable();
    let params: Vec<String> = required
        .iter()
        .chain(optional.iter())
        .map(|param| format!("{param}="))
        .collect();
    format!("{call_name}({})", params.join(", "))
}

async fn connect_http(server: &Server, url: &str) -> ToolResult<Connected> {
    let transport = StreamableHttpClientTransport::from_config(http_config(server, url)?);
    let service = tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport))
        .await
        .map_err(|_| {
            ToolError::msg(format!(
                "mcp server {:?}: cannot connect to {url}: timed out after {}s",
                server.name,
                CONNECT_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|error| {
            ToolError::msg(format!(
                "mcp server {:?}: cannot connect to {url}: {error}",
                server.name
            ))
        })?;
    connected(server, Arc::new(service), None).await
}

fn http_config(server: &Server, url: &str) -> ToolResult<StreamableHttpClientTransportConfig> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    let mut headers = HashMap::new();
    for (name, value) in &server.entry.headers {
        let parsed = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            ToolError::msg(format!(
                "mcp server {:?}: header name {name:?} is invalid",
                server.name
            ))
        })?;
        let value = HeaderValue::from_str(value).map_err(|_| {
            ToolError::msg(format!(
                "mcp server {:?}: header {name:?} has a value that is not a valid HTTP header",
                server.name
            ))
        })?;
        if parsed == AUTHORIZATION {
            if let Some(token) = value
                .to_str()
                .ok()
                .and_then(|value| value.strip_prefix("Bearer "))
            {
                config = config.auth_header(token);
                continue;
            }
        }
        headers.insert(parsed, value);
    }
    Ok(config.custom_headers(headers))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};

    use rmcp::handler::server::ServerHandler;
    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    };
    use rmcp::service::{RequestContext, RoleServer};
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    #[derive(Clone)]
    struct Echo;

    impl ServerHandler for Echo {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            Ok(ListToolsResult::with_all_items(vec![Tool::new(
                "echo",
                "Echo back.",
                Arc::new(Map::new()),
            )]))
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            Ok(CallToolResult::success(vec![rmcp::model::ContentBlock::text("pong")]).into())
        }
    }

    type Seen = Arc<std::sync::Mutex<Vec<(String, String)>>>;

    /// An in-process Streamable HTTP MCP server, and the headers it received.
    struct Http {
        url: String,
        sessions: Arc<LocalSessionManager>,
        headers: Seen,
    }

    async fn http_server() -> Http {
        let sessions = Arc::new(LocalSessionManager::default());
        let headers = Seen::default();
        let service = StreamableHttpService::new(
            || Ok(Echo),
            Arc::clone(&sessions),
            StreamableHttpServerConfig::default(),
        );
        let router = axum::Router::new().nest_service("/mcp", service).layer(
            axum::middleware::from_fn_with_state(Arc::clone(&headers), capture_headers),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Http {
            url: format!("http://{address}/mcp"),
            sessions,
            headers,
        }
    }

    async fn capture_headers(
        axum::extract::State(seen): axum::extract::State<Seen>,
        request: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        for name in ["x-api-key", "authorization"] {
            if let Some(value) = request
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
            {
                seen.lock()
                    .expect("headers")
                    .push((name.to_string(), value.to_string()));
            }
        }
        next.run(request).await
    }

    struct Fixture {
        _dir: TempDir,
        root: PathBuf,
        file: PathBuf,
    }

    fn fixture(servers: Value) -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("project");
        fs::create_dir(&root).expect("root");
        let file = dir.path().join("mcp.json");
        fs::write(&file, json!({ "mcpServers": servers }).to_string()).expect("config");
        Fixture {
            _dir: dir,
            root,
            file,
        }
    }

    fn registry(fixture: &Fixture) -> McpRegistry {
        McpRegistry::load_with(&fixture.root, Some(&fixture.file), &[], None)
    }

    #[test]
    fn a_gated_entry_needs_the_project_tool_name() {
        let fixture = fixture(json!({
            "photocraft": {"command": "figcraft-cli", "gated": true},
            "open": {"command": "figcraft-cli"},
        }));
        let names = |project_tools: Option<&[String]>| {
            McpRegistry::load_with(&fixture.root, Some(&fixture.file), &[], project_tools)
                .servers
                .iter()
                .map(|server| server.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(Some(&[])), vec!["open"]);
        assert_eq!(
            names(Some(&["photocraft".to_string()])),
            vec!["open", "photocraft"]
        );
        assert_eq!(names(None), vec!["open", "photocraft"]);
    }

    #[tokio::test]
    async fn an_http_server_answers_a_call_with_its_headers() {
        let server = http_server().await;
        let fixture = fixture(json!({"jobs": {
            "url": server.url,
            "headers": {"x-api-key": "secret", "authorization": "Bearer tok"},
        }}));
        let registry = registry(&fixture);

        let tools = registry.tools().await.expect("tools");
        let names: Vec<&str> = tools.iter().map(|tool| tool.call_name.as_str()).collect();
        assert_eq!(names, vec!["jobs.echo"]);
        let answer = registry.call("jobs.echo", &json!({})).await.expect("call");
        assert_eq!(answer, "pong");

        let seen = server.headers.lock().expect("headers").clone();
        assert!(
            seen.contains(&("x-api-key".to_string(), "secret".to_string())),
            "{seen:?}"
        );
        assert!(
            seen.contains(&("authorization".to_string(), "Bearer tok".to_string())),
            "{seen:?}"
        );
    }

    #[tokio::test]
    async fn an_entry_needs_exactly_one_of_command_and_url() {
        let cases = [
            (
                json!({"jobs": {"command": "x", "url": "https://example.com/mcp"}}),
                "mcp server \"jobs\": give a command or a url, not both",
            ),
            (
                json!({"jobs": {}}),
                "mcp server \"jobs\": give a command or a url",
            ),
            (
                json!({"jobs": {"url": "https://example.com/mcp", "args": ["-x"]}}),
                "mcp server \"jobs\": url entries cannot have args or setup",
            ),
            (
                json!({"jobs": {"url": "https://example.com/mcp", "setup": [["a"]]}}),
                "mcp server \"jobs\": url entries cannot have args or setup",
            ),
            (
                json!({"jobs": {"url": "ftp://example.com/mcp"}}),
                "mcp server \"jobs\": url must be an http(s) url such as https://host/mcp",
            ),
            (
                json!({"jobs": {"url": "not a url"}}),
                "mcp server \"jobs\": url must be an http(s) url such as https://host/mcp",
            ),
        ];
        for (servers, expected) in cases {
            let fixture = fixture(servers.clone());
            let registry = registry(&fixture);
            let error = registry
                .call("jobs.list", &json!({}))
                .await
                .expect_err("refused");
            assert_eq!(error.to_string(), expected, "{servers}");
        }
    }

    #[tokio::test]
    async fn an_unknown_transport_is_a_load_error() {
        let fixture = fixture(json!({
            "jobs": {"url": "https://example.com/mcp", "transport": "websocket"},
        }));
        let registry = registry(&fixture);
        let error = registry.tools().await.expect_err("refused");
        let text = error.to_string();
        assert!(text.contains("server \"jobs\""), "{text}");
        assert!(text.contains("unknown variant"), "{text}");
    }

    #[tokio::test]
    async fn an_unreachable_http_server_is_not_retried_for_thirty_seconds() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let accepts = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&accepts);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                drop(socket);
            }
        });
        let url = format!("http://{address}/mcp");
        let fixture = fixture(json!({
            "jobs": {"url": url, "headers": {"authorization": "Bearer topsecret"}},
        }));
        let registry = registry(&fixture);

        let error = registry
            .call("jobs.list", &json!({}))
            .await
            .expect_err("cannot connect");
        let text = error.to_string();
        assert!(text.contains(&format!("cannot connect to {url}")), "{text}");
        assert!(!text.contains("topsecret"), "header value leaked: {text}");
        let dialed = accepts.load(Ordering::SeqCst);
        assert!(dialed >= 1, "the client dialed the endpoint");

        let _ = registry.tools().await;
        let _ = registry.tools().await;
        assert_eq!(
            accepts.load(Ordering::SeqCst),
            dialed,
            "a failed url server is not dialed again within 30s"
        );
    }

    #[tokio::test]
    async fn an_expired_http_session_reconnects_for_the_next_call() {
        let server = http_server().await;
        let fixture = fixture(json!({"jobs": {"url": server.url}}));
        let registry = registry(&fixture);
        let first = registry.call("jobs.echo", &json!({})).await.expect("first");
        assert_eq!(first, "pong");

        server.sessions.sessions.write().await.clear();
        let after = registry
            .call("jobs.echo", &json!({}))
            .await
            .expect("a fresh session answers");
        assert_eq!(after, "pong");
    }
}
