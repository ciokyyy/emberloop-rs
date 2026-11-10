//! MCP client: one stdio child process per server in `<config root>/mcp.json`.
//! Its tools reach the model only through `execute`; the harness tool list is
//! fixed, so a server is never visible as a tool of its own.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::RwLock;

use crate::figcraft;
use crate::graph;
use crate::{ToolError, ToolResult};

mod design;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

fn yes() -> bool {
    true
}

/// One `mcpServers` entry. `url` is parsed only so it can be refused by name.
#[derive(Clone, Debug, Deserialize)]
struct Entry {
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
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
    url: Option<String>,
}

struct Server {
    name: String,
    entry: Entry,
    /// Why this server cannot be used; it still answers with the reason.
    error: Option<String>,
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

pub struct McpRegistry {
    root: PathBuf,
    servers: Vec<Server>,
    /// A config that would not load; it costs the MCP tools and nothing else.
    error: Option<String>,
    connected: RwLock<BTreeMap<String, Arc<Connected>>>,
    /// Resolved `project` per server, so `list_projects` runs once.
    projects: RwLock<BTreeMap<String, String>>,
}

impl McpRegistry {
    pub fn load(root: &Path, file: Option<&Path>) -> Self {
        Self::load_with(root, file, &graph::default_bin_dirs())
    }

    /// `bin_dirs` is searched for a native codebase-memory-mcp that replaces the npx fallback.
    pub fn load_with(root: &Path, file: Option<&Path>, bin_dirs: &[PathBuf]) -> Self {
        let mut registry = Self {
            root: root.to_path_buf(),
            servers: Vec::new(),
            error: None,
            connected: RwLock::new(BTreeMap::new()),
            projects: RwLock::new(BTreeMap::new()),
        };
        if let Some(file) = file {
            if let Err(error) = registry.read(file, bin_dirs) {
                registry.error = Some(error);
            }
        } else if let Ok(entry) = serde_json::from_value::<Entry>(figcraft::default_entry()) {
            registry.servers.push(Server {
                name: figcraft::SERVER.to_string(),
                entry,
                error: None,
            });
        }
        registry
    }

    fn read(&mut self, file: &Path, bin_dirs: &[PathBuf]) -> Result<(), String> {
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
            let error = match &entry.command {
                Some(command) if !command.trim().is_empty() && entry.url.is_none() => None,
                _ => Some(format!(
                    "mcp server {name:?}: only stdio servers are supported, so give a command and not a url"
                )),
            };
            self.servers.push(Server {
                name: name.clone(),
                entry,
                error,
            });
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
        let mut tools = Vec::new();
        for server in &self.servers {
            if server.error.is_some() {
                continue;
            }
            match self.ensure(&server.name).await {
                Ok(connected) => tools.extend(server_tools(server, &connected.tools)),
                Err(error) => eprintln!("{error}"),
            }
        }
        Ok(tools)
    }

    /// Calls `server.tool`; a server's own error text becomes our error.
    pub async fn call(&self, name: &str, args: &Value) -> ToolResult<String> {
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
            self.resync(server, &connected).await?;
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
        let mut connected = self.connected.write().await;
        if let Some(ready) = connected.get(name) {
            return Ok(Arc::clone(ready));
        }
        let server = self
            .servers
            .iter()
            .find(|it| it.name == name)
            .ok_or_else(|| ToolError::msg(format!("unknown tool {name:?}")))?;
        if let Some(error) = &server.error {
            return Err(ToolError::msg(error.clone()));
        }
        let ready = Arc::new(connect(&self.root, server).await?);
        if name == figcraft::SERVER {
            self.connect_design(server, &ready).await?;
        }
        connected.insert(name.to_string(), Arc::clone(&ready));
        Ok(ready)
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
    let service = Arc::new(service);
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
