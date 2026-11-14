use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;

use crate::{McpTool, ToolError, ToolResult, Toolbox};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const TRACE_TAIL_LINES: usize = 30;
const LAUNCHER: &str = "import sys; sys.path.insert(0, sys.argv[1]); import runpy; runpy.run_path(sys.argv[2], run_name='__main__')";

#[derive(Deserialize)]
struct ExecuteArgs {
    code: String,
    #[serde(default)]
    timeout: Option<u64>,
}

#[derive(Deserialize)]
struct BridgeRequest {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    op: String,
    #[serde(default)]
    args: Value,
}

#[derive(Serialize)]
struct BridgeResponse {
    id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

struct BridgeState {
    session_dir: PathBuf,
    exec_id: u64,
    pending_store: Mutex<HashMap<String, Value>>,
}

impl BridgeState {
    fn create(spill_dir: &Path, code: &str) -> std::io::Result<(Self, PathBuf)> {
        let session_dir = spill_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| spill_dir.to_path_buf());
        let exec_dir = session_dir.join("exec");
        fs::create_dir_all(&exec_dir)?;
        let max_id = fs::read_dir(&exec_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_suffix(".py")?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap_or(0);
        for offset in 0..1000 {
            let id = max_id + 1 + offset;
            let path = exec_dir.join(format!("{id}.py"));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    std::io::Write::write_all(&mut file, code.as_bytes())?;
                    let state = Self {
                        session_dir,
                        exec_id: id,
                        pending_store: Mutex::new(HashMap::new()),
                    };
                    return Ok((state, path));
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "failed to allocate exec id",
        ))
    }

    fn store_path(&self) -> PathBuf {
        self.session_dir.join("execute_store.json")
    }

    fn scripts_dir(&self, scope: &str) -> PathBuf {
        self.session_dir.join("scripts").join(scope)
    }

    fn exec_dir(&self) -> PathBuf {
        self.session_dir.join("exec")
    }

    fn exec_path(&self, id: u64) -> PathBuf {
        self.exec_dir().join(format!("{id}.py"))
    }

    fn load_store_map(&self) -> HashMap<String, Value> {
        fs::read_to_string(self.store_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn handle_op(&self, op: &str, args: &Value) -> Result<String, String> {
        match op {
            "store" => self.op_store(args),
            "load" => self.op_load(args),
            "save" => self.op_save(args),
            "scripts" => self.op_scripts(),
            "script" => self.op_script(args),
            "delete" => self.op_delete(args),
            "exec_source" => self.op_exec_source(args),
            "update_exec" => self.op_update_exec(args),
            other => Err(format!("unknown execute operation {other:?}")),
        }
    }

    fn op_store(&self, args: &Value) -> Result<String, String> {
        let key = args
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !valid_store_key(key) {
            return Err(format!(
                "invalid store key {key:?}: use [a-z][a-z0-9_.-]{{0,63}}"
            ));
        }
        let val = args.get("value").cloned().unwrap_or(Value::Null);
        let encoded = serde_json::to_string(&val).map_err(|e| e.to_string())?;
        if encoded.len() > 256 * 1024 {
            return Err(format!("value for {key:?} exceeds 256 KiB cap"));
        }
        let mut pending = self.pending_store.lock().unwrap_or_else(|e| e.into_inner());
        pending.insert(key.to_string(), val);
        Ok(format!("stored {key}"))
    }

    fn op_load(&self, args: &Value) -> Result<String, String> {
        let key = args
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !valid_store_key(key) {
            return Err(format!(
                "invalid store key {key:?}: use [a-z][a-z0-9_.-]{{0,63}}"
            ));
        }
        let pending = self.pending_store.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(val) = pending.get(key) {
            return Ok(json!({ "found": true, "value": val }).to_string());
        }
        drop(pending);
        let base = self.load_store_map();
        if let Some(val) = base.get(key) {
            return Ok(json!({ "found": true, "value": val }).to_string());
        }
        Ok(json!({ "found": false }).to_string())
    }

    fn op_save(&self, args: &Value) -> Result<String, String> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !valid_script_name(name) {
            return Err(format!(
                "invalid script name {name:?}: use [a-z][a-z0-9_]{{0,47}}"
            ));
        }
        let code = args.get("code").and_then(Value::as_str).unwrap_or_default();
        if code.trim().is_empty() {
            return Err("script source is empty".to_string());
        }
        let desc = args
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or("")
            .trim();
        let scope = args
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("project");
        if scope != "project" && scope != "user" {
            return Err("scope is project or user".to_string());
        }
        let dir = self.scripts_dir(scope);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let header = if desc.is_empty() {
            format!("saved script {name}")
        } else {
            desc.to_string()
        };
        let body = format!("# description: {header}\n{code}\n");
        fs::write(dir.join(format!("{name}.py")), body).map_err(|e| e.to_string())?;
        Ok(format!("saved {name} ({scope} scope)"))
    }

    fn op_scripts(&self) -> Result<String, String> {
        let mut list = Vec::new();
        for scope in ["project", "user"] {
            let dir = self.scripts_dir(scope);
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let Some(name) = fname.strip_suffix(".py") else {
                    continue;
                };
                if !valid_script_name(name) {
                    continue;
                }
                let raw = fs::read_to_string(entry.path()).unwrap_or_default();
                let desc = raw
                    .lines()
                    .find_map(|l| l.strip_prefix("# description:").map(str::trim))
                    .unwrap_or("")
                    .to_string();
                list.push(json!({
                    "name": name,
                    "description": desc,
                    "scope": scope,
                }));
            }
        }
        serde_json::to_string(&list).map_err(|e| e.to_string())
    }

    fn op_script(&self, args: &Value) -> Result<String, String> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !valid_script_name(name) {
            return Err(format!("invalid script name {name:?}"));
        }
        for scope in ["project", "user"] {
            let path = self.scripts_dir(scope).join(format!("{name}.py"));
            if let Ok(source) = fs::read_to_string(path) {
                return Ok(json!({ "source": source }).to_string());
            }
        }
        Err(format!("no saved script {name:?}"))
    }

    fn op_delete(&self, args: &Value) -> Result<String, String> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let scope = args
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("project");
        let path = self.scripts_dir(scope).join(format!("{name}.py"));
        fs::remove_file(path).map_err(|_| format!("no saved script {name:?} in {scope} scope"))?;
        Ok(format!("deleted {name}"))
    }
    fn op_exec_source(&self, args: &Value) -> Result<String, String> {
        let id = args
            .get("id")
            .and_then(Value::as_u64)
            .filter(|&id| id >= 1)
            .ok_or_else(|| "invalid exec id".to_string())?;
        let source =
            fs::read_to_string(self.exec_path(id)).map_err(|_| format!("no saved exec #{id}"))?;
        Ok(json!({ "source": source }).to_string())
    }

    fn op_update_exec(&self, args: &Value) -> Result<String, String> {
        let code = args.get("code").and_then(Value::as_str).unwrap_or_default();
        if code.trim().is_empty() {
            return Err("script source is empty".to_string());
        }
        fs::create_dir_all(self.exec_dir()).map_err(|e| e.to_string())?;
        fs::write(self.exec_path(self.exec_id), code).map_err(|e| e.to_string())?;
        Ok(format!("updated exec #{}", self.exec_id))
    }

    fn flush(&self) {
        let pending = self.pending_store.lock().unwrap_or_else(|e| e.into_inner());
        if pending.is_empty() {
            return;
        }
        let mut map = self.load_store_map();
        for (k, v) in pending.iter() {
            map.insert(k.clone(), v.clone());
        }
        if let Ok(raw) = serde_json::to_string(&map) {
            let _ = fs::create_dir_all(&self.session_dir);
            let _ = fs::write(self.store_path(), raw);
        }
    }
}

fn valid_store_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && key.len() <= 64
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
}

fn valid_script_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && name.len() <= 48
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

pub fn run<'a>(
    toolbox: Arc<Toolbox>,
    args: &'a Value,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult<String>> + Send + 'a>> {
    Box::pin(async move {
        if crate::IN_SCRIPT.try_with(|_| ()).is_ok() {
            return Err(ToolError::msg(
                "execute cannot be called recursively inside an execute script",
            ));
        }
        let args: ExecuteArgs = serde_json::from_value(args.clone())?;
        if args.code.trim().is_empty() {
            return Err(ToolError::msg("code is required"));
        }
        let timeout_secs = toolbox.call_secs(args.timeout, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
        let tmp_dir = create_bridge_dir()?;
        // An MCP failure or a slow start costs the MCP stubs only: the builtin ones stay.
        let mcp = toolbox.mcp_tools_soon().await;
        let result = run_in_bridge(
            Arc::clone(&toolbox),
            &tmp_dir,
            &args.code,
            timeout_secs,
            &mcp,
        )
        .await;
        let _ = fs::remove_dir_all(&tmp_dir);
        result
    })
}

fn create_bridge_dir() -> ToolResult<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("el-exec-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

async fn run_in_bridge(
    toolbox: Arc<Toolbox>,
    dir: &Path,
    code: &str,
    timeout_secs: u64,
    mcp: &[McpTool],
) -> ToolResult<String> {
    let (state, exec_path) = BridgeState::create(toolbox.spill_dir(), code)?;
    let exec_id = state.exec_id;
    let sock_path = dir.join("sock");
    let stub = build_tools_py(&sock_path, &mcp_stub(mcp), &exec_path);
    fs::write(dir.join("tools.py"), stub)?;

    let listener = UnixListener::bind(&sock_path)?;
    let state = Arc::new(state);
    let tb_for_bridge = Arc::clone(&toolbox);
    let state_for_bridge = Arc::clone(&state);
    let bridge_task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let tb = Arc::clone(&tb_for_bridge);
            let st = Arc::clone(&state_for_bridge);
            tokio::spawn(async move {
                serve_conn(tb, st, stream).await;
            });
        }
    });

    let py = std::env::var("EL_PYTHON")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "python3".to_string());
    // The runtime env file must reach this process too, not only `bash`: `PATH` additions are
    // useless here if a script's own `subprocess` cannot find the command.
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(format!("{}exec \"$@\"", toolbox.env_prelude()))
        .arg("sh")
        .arg(&py)
        .arg("-I")
        .arg("-c")
        .arg(LAUNCHER)
        .arg(dir)
        .arg(&exec_path)
        .current_dir(toolbox.root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd
        .spawn()
        .map_err(|e| ToolError::msg(format!("spawn python3: {e}")))?;
    let pid = child.id();
    let wait_res =
        tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;
    bridge_task.abort();

    match wait_res {
        Ok(Ok(output)) => {
            let exit_code = output.status.code().unwrap_or(-1);
            if exit_code == 0 {
                state.flush();
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            format_output(
                &toolbox,
                code,
                &stdout,
                &stderr,
                ExecOutcome::Exit(exit_code),
                exec_id,
            )
        }
        Ok(Err(e)) => Err(ToolError::Io(e)),
        Err(_) => {
            kill_group(pid);
            format_output(
                &toolbox,
                code,
                "",
                "",
                ExecOutcome::TimedOut(timeout_secs),
                exec_id,
            )
        }
    }
}

#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // SAFETY: negative pid targets the process group created by process_group(0).
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: Option<u32>) {}

async fn serve_conn(toolbox: Arc<Toolbox>, state: Arc<BridgeState>, stream: UnixStream) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let req: BridgeRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = write_resp(
                    &mut writer,
                    BridgeResponse {
                        id: 0,
                        result: None,
                        error: Some(format!("bad bridge request: {e}")),
                    },
                )
                .await;
                continue;
            }
        };
        let resp = if !req.op.is_empty() {
            match state.handle_op(&req.op, &req.args) {
                Ok(res) => BridgeResponse {
                    id: req.id,
                    result: Some(res),
                    error: None,
                },
                Err(err) => BridgeResponse {
                    id: req.id,
                    result: None,
                    error: Some(err),
                },
            }
        } else if req.tool == "execute" {
            BridgeResponse {
                id: req.id,
                result: None,
                error: Some("execute cannot be called recursively".to_string()),
            }
        } else {
            match crate::IN_SCRIPT
                .scope((), toolbox.call_inner(&req.tool, &req.args))
                .await
            {
                Ok(res) => BridgeResponse {
                    id: req.id,
                    result: Some(res),
                    error: None,
                },
                Err(err) => BridgeResponse {
                    id: req.id,
                    result: None,
                    error: Some(err.to_string()),
                },
            }
        };
        if write_resp(&mut writer, resp).await.is_err() {
            break;
        }
    }
}

async fn write_resp(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    resp: BridgeResponse,
) -> std::io::Result<()> {
    let encoded = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string());
    writer.write_all(encoded.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

enum ExecOutcome {
    Exit(i32),
    TimedOut(u64),
}

fn format_output(
    toolbox: &Toolbox,
    code: &str,
    stdout: &str,
    stderr: &str,
    outcome: ExecOutcome,
    exec_id: u64,
) -> ToolResult<String> {
    let stderr_part = match outcome {
        ExecOutcome::Exit(code) if code != 0 => tail_lines(stderr, TRACE_TAIL_LINES),
        _ => stderr.to_string(),
    };
    let mut text = stdout.to_string();
    if !stderr_part.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&stderr_part);
    }
    let first_line = code.lines().next().unwrap_or("").trim();
    let mut text = toolbox.maybe_spill(text, &format!("execute {first_line}"))?;
    let tag = match outcome {
        ExecOutcome::TimedOut(secs) => format!("[timed out after {secs}s; saved exec #{exec_id}]"),
        ExecOutcome::Exit(code) if code != 0 => {
            format!("[exit status {code}; saved exec #{exec_id}]")
        }
        ExecOutcome::Exit(_) => format!("[saved exec #{exec_id}]"),
    };
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&tag);
    Ok(text)
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= n {
        return s.to_string();
    }
    lines[lines.len() - n..].join("\n")
}

/// The MCP tool list `tools.py` loads: specs for `search`/`help`, summaries for
/// `servers()`, and a bare alias for each name only one server exposes.
fn mcp_stub(tools: &[McpTool]) -> String {
    let mut specs = Map::new();
    let mut servers = Map::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for tool in tools {
        specs.insert(
            tool.call_name.clone(),
            json!({"sig": tool.signature, "desc": tool.description}),
        );
        servers
            .entry(tool.server.clone())
            .or_insert_with(|| json!(tool.summary));
        *counts.entry(tool.name.as_str()).or_default() += 1;
    }
    let mut aliases = Map::new();
    for tool in tools {
        if counts.get(tool.name.as_str()) == Some(&1) {
            aliases.insert(tool.name.clone(), json!(tool.call_name));
        }
    }
    let payload = json!({"specs": specs, "servers": servers, "aliases": aliases}).to_string();
    // A JSON string literal is also a Python one, so the stub can json.loads it.
    serde_json::to_string(&payload).unwrap_or_else(|_| "\"{}\"".to_string())
}

fn build_tools_py(sock_path: &Path, mcp_json: &str, exec_path: &Path) -> String {
    let sock_json = serde_json::to_string(&sock_path.to_string_lossy().to_string())
        .unwrap_or_else(|_| "\"\"".to_string());
    let exec_path_json = serde_json::to_string(&exec_path.to_string_lossy().to_string())
        .unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"import json as _json
import linecache as _linecache
import socket as _socket
import sys as _sys
import threading as _threading
from concurrent.futures import ThreadPoolExecutor as _ThreadPoolExecutor

class ToolError(Exception):
    pass

_SOCKET_PATH = {sock_json}
_EXEC_PATH = {exec_path_json}
_CALL_LIMIT = 500
_call_count = 0
_next_id = 0
_lock = _threading.Lock()
_sock = None
_reader = None
_run_depth = 0

_SPECS = {{
    "read": {{"sig": "read(path, limit, offset)", "desc": "Read a file, directory, ctx: handle, or URL."}},
    "grep": {{"sig": "grep(pattern, case_insensitive, context, glob, max_results, output_mode, path)", "desc": "Search files with regex."}},
    "edit": {{"sig": "edit(input)", "desc": "Apply line-anchored hashline edits."}},
    "write": {{"sig": "write(path, content)", "desc": "Write a whole file inside the project root."}},
    "bash": {{"sig": "bash(command, background, timeout)", "desc": "Run a shell command in the project root."}},
    "background": {{"sig": "background(action, id, offset, timeout)", "desc": "Manage background commands."}},
    "find": {{"sig": "find(query, grep_keywords, limit, mode, path)", "desc": "Semantic/lexical code finder with modes."}},
    "websearch": {{"sig": "websearch(query, count, domains, fresh, freshness, provider)", "desc": "Search the web."}},
    "spawn": {{"sig": "spawn(role, text, title, tier, ticketPath, startingPoint)", "desc": "Start a Worker or Scout or start/reuse a Spec or Standards reviewer."}},
    "steer": {{"sig": "steer(id, text, tier, ticketPath, startingPoint)", "desc": "Continue an existing subagent Session."}},
    "stop": {{"sig": "stop(id)", "desc": "Stop a subagent without deleting its conversation."}},
    "result": {{"sig": "result(id)", "desc": "Retrieve all reports from a subagent."}},
    "submit": {{"sig": "submit(status, report, summary)", "desc": "Send a new report to the parent Session."}},
    "retry_call": {{"sig": "retry_call(id, set, patches)", "desc": "Retry failed call #id with field overrides or string patches."}},
}}

_MCP = _json.loads({mcp_json})
_SPECS.update(_MCP["specs"])

def _next():
    global _next_id
    with _lock:
        _next_id += 1
        return _next_id

def _count_call():
    global _call_count
    with _lock:
        _call_count += 1
        n = _call_count
    if n > _CALL_LIMIT:
        raise ToolError("this script has made %d tool calls: the cap is %d. Split the work into more execute calls." % (_CALL_LIMIT, _CALL_LIMIT))

def _exchange(sock, reader, payload):
    payload["id"] = _next()
    sock.sendall((_json.dumps(payload) + "\n").encode("utf-8"))
    line = reader.readline()
    if not line:
        raise ToolError("execute bridge closed")
    resp = _json.loads(line)
    if resp.get("error"):
        raise ToolError(resp["error"])
    return resp.get("result", "")

def _call(tool, args):
    _count_call()
    s = _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM)
    try:
        s.connect(_SOCKET_PATH)
        return _exchange(s, s.makefile("r"), {{"tool": tool, "args": args}})
    finally:
        s.close()

def _op(op, args):
    s = _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM)
    try:
        s.connect(_SOCKET_PATH)
        return _exchange(s, s.makefile("r"), {{"op": op, "args": args}})
    finally:
        s.close()

def call(tool, **kwargs):
    return _call(tool, kwargs)

def read(**kwargs): return _call("read", kwargs)
def grep(**kwargs): return _call("grep", kwargs)
def edit(**kwargs): return _call("edit", kwargs)
def write(**kwargs): return _call("write", kwargs)
def bash(**kwargs): return _call("bash", kwargs)
def background(**kwargs): return _call("background", kwargs)
def find(**kwargs): return _call("find", kwargs)
def websearch(**kwargs): return _call("websearch", kwargs)

def parallel(calls, max_workers=4):
    items = [(c[0], c[1] if len(c) > 1 else {{}}) for c in calls]
    if not items:
        return []
    workers = max(1, min(int(max_workers or 4), 8, len(items)))
    def one(item):
        name, kwargs = item
        if name in ("edit", "write", "multiedit", "str_replace"):
            return ToolError("%s is refused inside tools.parallel: two writes can race on one file" % name)
        try:
            return _call(name, kwargs)
        except ToolError as e:
            return e
    with _ThreadPoolExecutor(max_workers=workers) as pool:
        return list(pool.map(one, items))

def store(key, value):
    return _op("store", {{"key": key, "value": value}})

def load(key, default=None):
    data = _json.loads(_op("load", {{"key": key}}))
    if not data.get("found"):
        return default
    return data.get("value", default)

def save(name, code, description, scope="project"):
    return _op("save", {{"name": name, "code": code, "description": description, "scope": scope}})

def delete(name, scope="project"):
    return _op("delete", {{"name": name, "scope": scope}})

def scripts():
    return _json.loads(_op("scripts", {{}}))

def servers():
    return _MCP["servers"]

def run(name, **args):
    global _run_depth
    if _run_depth >= 4:
        raise ToolError("saved scripts nest at most 4 deep")
    source = _json.loads(_op("script", {{"name": name}})).get("source", "")
    scope = {{"tools": _sys.modules[__name__], "args": args, "result": None}}
    _run_depth += 1
    try:
        exec(compile(source, "<script %s>" % name, "exec"), scope)
    finally:
        _run_depth -= 1
    return scope.get("result")

def _parse_exec_id(exec_id):
    try:
        eid = int(str(exec_id).strip().lstrip('#'))
    except (TypeError, ValueError):
        raise ToolError("invalid exec id %r" % (exec_id,))
    if eid < 1:
        raise ToolError("invalid exec id %r" % (exec_id,))
    return eid

def exec_source(exec_id):
    eid = _parse_exec_id(exec_id)
    return _json.loads(_op("exec_source", {{"id": eid}})).get("source", "")

def rerun(exec_id, patches=None):
    global _run_depth
    if _run_depth >= 4:
        raise ToolError("saved scripts nest at most 4 deep")
    eid = _parse_exec_id(exec_id)
    source = _json.loads(_op("exec_source", {{"id": eid}})).get("source", "")
    if patches:
        items = list(patches.items()) if isinstance(patches, dict) else list(patches)
        for pair in items:
            if not isinstance(pair, (list, tuple)) or len(pair) != 2:
                raise ToolError("each patch must be an (old, new) pair")
            old, new = pair
            if not isinstance(old, str) or not isinstance(new, str) or not old:
                raise ToolError("patch old/new must be strings and old non-empty")
            count = source.count(old)
            if count == 0:
                raise ToolError("patch target not found in exec #%d: %r" % (eid, old[:80]))
            if count > 1:
                raise ToolError("patch target matched %d times in exec #%d; add more context" % (count, eid))
            source = source.replace(old, new, 1)
    if _EXEC_PATH and _run_depth == 0:
        _op("update_exec", {{"code": source}})
        _linecache.checkcache(_EXEC_PATH)
    scope = {{"__name__": "__main__", "tools": _sys.modules[__name__], "result": None}}
    _run_depth += 1
    try:
        exec(compile(source, _EXEC_PATH or ("<exec #%d>" % eid), "exec"), scope)
    finally:
        _run_depth -= 1
    return scope.get("result")

def search(query=""):
    q = (query or "").lower()
    out = []
    for name in sorted(_SPECS):
        spec = _SPECS[name]
        if q in name.lower() or q in spec["desc"].lower():
            out.append("%s: %s" % (spec["sig"], spec["desc"]))
    if not out:
        return "no tools match %r; available: %s" % (query, ", ".join(sorted(_SPECS)))
    return "\n".join(out)

def help(name):
    name = _MCP["aliases"].get(name, name)
    spec = _SPECS.get(name)
    if spec is None:
        return "unknown tool %r; available: %s" % (name, ", ".join(sorted(_SPECS)))
    return "%s:\n%s" % (spec["sig"], spec["desc"])

def __getattr__(name):
    target = _MCP["aliases"].get(name)
    if target is None:
        raise AttributeError("module %r has no attribute %r" % (__name__, name))
    return lambda **kwargs: _call(target, kwargs)
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolConfig;

    #[tokio::test]
    async fn execute_runs_python_and_calls_tools_over_bridge() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let tb = Arc::new(
            Toolbox::new(ToolConfig::new(dir.path(), spill.path().join("spill"))).unwrap(),
        );

        let out = run(
            Arc::clone(&tb),
            &json!({
                "code": "import tools\nres = tools.read(path='notes.txt')\ntools.store('k1', {'lines': len(res.splitlines())})\nprint(res)"
            }),
        )
        .await
        .unwrap();
        assert!(out.contains("alpha"), "got: {out}");

        let loaded = run(
            Arc::clone(&tb),
            &json!({
                "code": "import tools\nprint(tools.load('k1'))"
            }),
        )
        .await
        .unwrap();
        assert!(loaded.contains("lines"), "got: {loaded}");
    }

    #[tokio::test]
    async fn execute_parallel_runs_read_calls_and_refuses_write() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let tb = Arc::new(
            Toolbox::new(ToolConfig::new(dir.path(), spill.path().join("spill"))).unwrap(),
        );

        let out = run(
            Arc::clone(&tb),
            &json!({
                "code": "import tools\nres = tools.parallel([('read', {'path': 'a.txt'}), ('write', {'path': 'b.txt', 'content': 'x'})])\nprint(res[0])\nprint(type(res[1]).__name__)"
            }),
        )
        .await
        .unwrap();
        assert!(out.contains("hello"), "got: {out}");
        assert!(out.contains("ToolError"), "got: {out}");
    }

    #[tokio::test]
    async fn execute_loads_the_runtime_env_file() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let env = dir.path().join("env");
        fs::write(&env, "export EMBERLOOP_TEST_TOKEN=abc\n").unwrap();
        let tb = Arc::new(
            Toolbox::new(ToolConfig {
                env_file: Some(env),
                ..ToolConfig::new(dir.path(), spill.path().join("spill"))
            })
            .unwrap(),
        );

        let out = run(
            Arc::clone(&tb),
            &json!({"code": "import os\nprint(os.environ.get('EMBERLOOP_TEST_TOKEN'))"}),
        )
        .await
        .unwrap();
        assert!(out.contains("abc"), "got: {out}");
    }

    #[tokio::test]
    async fn execute_saves_scripts_and_reruns_with_patches() {
        let dir = tempfile::tempdir().unwrap();
        let session = tempfile::tempdir().unwrap();
        let tb = Arc::new(
            Toolbox::new(ToolConfig::new(dir.path(), session.path().join("spill"))).unwrap(),
        );

        let first = run(
            Arc::clone(&tb),
            &json!({
                "code": "a = 10\nb = 0\nc = 5\nprint(a // b)\nprint(a // c)"
            }),
        )
        .await
        .unwrap();
        assert!(
            first.contains("[exit status 1; saved exec #1]"),
            "got: {first}"
        );
        assert!(session.path().join("exec/1.py").exists());

        let second = run(
            Arc::clone(&tb),
            &json!({
                "code": "import tools\ntools.rerun(1, patches=[('b = 0', 'b = 2'), ('c = 5', 'c = 0')])"
            }),
        )
        .await
        .unwrap();
        assert!(second.contains("5"), "got: {second}");
        assert!(
            second.contains("[exit status 1; saved exec #2]"),
            "got: {second}"
        );
        // #2 persisted the patched source so a follow-up rerun builds on it.
        let saved_two = fs::read_to_string(session.path().join("exec/2.py")).unwrap();
        assert!(saved_two.contains("b = 2"), "got: {saved_two}");

        let third = run(
            Arc::clone(&tb),
            &json!({
                "code": "import tools\ntools.rerun(2, patches=[('c = 0', 'c = 5')])"
            }),
        )
        .await
        .unwrap();
        assert!(third.contains("5\n2"), "got: {third}");
        assert!(third.contains("[saved exec #3]"), "got: {third}");
    }

    #[tokio::test]
    async fn execute_rerun_rejects_missing_or_ambiguous_patches_and_exposes_source() {
        let dir = tempfile::tempdir().unwrap();
        let session = tempfile::tempdir().unwrap();
        let tb = Arc::new(
            Toolbox::new(ToolConfig::new(dir.path(), session.path().join("spill"))).unwrap(),
        );

        let first = run(
            Arc::clone(&tb),
            &json!({"code": "x = 1\ny = 1\nprint(x + y)"}),
        )
        .await
        .unwrap();
        assert!(first.contains("[saved exec #1]"), "got: {first}");

        let missing = run(
            Arc::clone(&tb),
            &json!({"code": "import tools\ntools.rerun(1, patches=[('nope', 'z')])"}),
        )
        .await
        .unwrap();
        assert!(missing.contains("patch target not found"), "got: {missing}");
        assert!(
            missing.contains("[exit status 1; saved exec #2]"),
            "got: {missing}"
        );

        let ambiguous = run(
            Arc::clone(&tb),
            &json!({"code": "import tools\ntools.rerun(1, patches=[('= 1', '= 2')])"}),
        )
        .await
        .unwrap();
        assert!(ambiguous.contains("matched 2 times"), "got: {ambiguous}");

        let inspected = run(
            Arc::clone(&tb),
            &json!({"code": "import tools\nprint(tools.exec_source(1).splitlines()[2])"}),
        )
        .await
        .unwrap();
        assert!(inspected.contains("print(x + y)"), "got: {inspected}");
    }
}
