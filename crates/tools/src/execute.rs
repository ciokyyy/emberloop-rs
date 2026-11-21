use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex as AsyncMutex;

use crate::{McpTool, ToolError, ToolResult, Toolbox};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const TRACE_TAIL_LINES: usize = 30;
const AUTO_RETURN_CAP: usize = 2048;
const LAUNCHER: &str = "import sys, runpy; sys.path.insert(0, sys.argv[1]); import tools; runpy.run_path(sys.argv[2], run_name='__main__', init_globals={'tools': tools})";

#[derive(Deserialize)]
struct ExecuteArgs {
    #[serde(default)]
    code: String,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    background: bool,
    #[serde(default)]
    reset: bool,
}

#[derive(Deserialize)]
struct BridgeRequest {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    exec_id: u64,
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

#[derive(Clone, Serialize, Deserialize)]
struct ExecChild {
    name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    target: String,
    ms: u64,
    status: String,
}

/// MIME type and base64 data of an image a script read.
pub type Image = (&'static str, String);

struct BridgeState {
    session_dir: PathBuf,
    exec_id: u64,
    pending_store: Mutex<HashMap<String, Value>>,
    children: Mutex<Vec<ExecChild>>,
    /// Images the script read: the bridge answers text only, so the execute result carries them.
    images: Mutex<Vec<Image>>,
    /// A background script has no caller left to flush on exit 0, so each `store` lands now.
    flush_store: bool,
}

impl BridgeState {
    fn create(spill_dir: &Path, code: &str, flush_store: bool) -> std::io::Result<(Self, PathBuf)> {
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
                        children: Mutex::new(Vec::new()),
                        images: Mutex::new(Vec::new()),
                        flush_store,
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

    fn children_path(&self) -> PathBuf {
        self.exec_dir()
            .join(format!("{}.children.json", self.exec_id))
    }
    fn load_store_map(&self) -> HashMap<String, Value> {
        fs::read_to_string(self.store_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn take_images(&self) -> Vec<Image> {
        std::mem::take(&mut *self.images.lock().unwrap_or_else(|e| e.into_inner()))
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
        if self.flush_store {
            drop(pending);
            self.flush();
        }
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
        let id = match args.get("id").and_then(Value::as_u64) {
            Some(id) if id > 0 => id,
            _ => self.exec_id,
        };
        fs::create_dir_all(self.exec_dir()).map_err(|e| e.to_string())?;
        fs::write(self.exec_path(id), code).map_err(|e| e.to_string())?;
        Ok(json!({ "id": id }).to_string())
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
    fn record_child(&self, tool: &str, args: &Value, ms: u64, ok: bool) {
        let child = ExecChild {
            name: tool.to_string(),
            target: child_target(tool, args),
            ms,
            status: if ok { "completed" } else { "failed" }.to_string(),
        };
        let mut list = self.children.lock().unwrap_or_else(|e| e.into_inner());
        list.push(child);
    }

    fn write_children(&self) {
        let list = self.children.lock().unwrap_or_else(|e| e.into_inner());
        if list.is_empty() {
            return;
        }
        if let Ok(raw) = serde_json::to_string(&*list) {
            let _ = fs::create_dir_all(self.exec_dir());
            let _ = fs::write(self.children_path(), raw);
        }
    }
}

fn child_target(tool: &str, args: &Value) -> String {
    let Some(obj) = args.as_object() else {
        return String::new();
    };
    let pick_str = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| obj.get(*k).and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    };
    let raw = match tool {
        "edit" => pick_str(&["path", "file_path", "filePath"]).or_else(|| {
            let input = obj.get("input").and_then(Value::as_str)?;
            let headers = crate::edit::headers(input);
            let (first, rest) = headers.split_first()?;
            Some(match rest.len() {
                0 => first.trim().to_string(),
                more => format!("{} (+{more} more)", first.trim()),
            })
        }),
        "read" | "write" => pick_str(&["path", "file_path", "filePath", "pattern", "glob"]),
        "grep" => pick_str(&["pattern", "regex", "query"]),
        "bash" => pick_str(&["command", "cmd", "script"]),
        "find" | "websearch" => pick_str(&["query", "pattern"]),
        "ctx_search" => obj
            .get("queries")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|s| !s.is_empty())
            .or_else(|| pick_str(&["query"])),
        _ => pick_str(&[
            "path",
            "file_path",
            "command",
            "query",
            "pattern",
            "url",
            "name",
            "id",
        ]),
    };
    let Some(text) = raw else {
        return String::new();
    };
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= 80 {
        line.to_string()
    } else {
        let mut out: String = line.chars().take(79).collect();
        out.push('…');
        out
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

type RunParts<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = ToolResult<(String, Vec<Image>)>> + Send + 'a>,
>;

/// Runs a script and returns its rendered output plus every image it read in `tools.read`.
pub fn run_parts<'a>(toolbox: Arc<Toolbox>, args: &'a Value) -> RunParts<'a> {
    Box::pin(async move {
        if crate::IN_SCRIPT.try_with(|_| ()).is_ok() {
            return Err(ToolError::msg(
                "nested execute is not supported inside an execute script",
            ));
        }
        let args: ExecuteArgs = serde_json::from_value(args.clone())?;
        if args.background && args.reset {
            return Err(ToolError::msg("reset cannot be combined with background"));
        }
        if args.code.trim().is_empty() && !args.reset {
            return Err(ToolError::msg("code is required"));
        }
        // An MCP failure or a slow start costs the MCP stubs only: the builtin ones stay.
        let mcp = toolbox.mcp_tools_soon().await;
        if args.background {
            // shortcut: a background job outlives this call, so images it reads are dropped.
            return run_background(&toolbox, &args.code, &mcp)
                .await
                .map(|text| (text, Vec::new()));
        }
        let timeout_secs = toolbox.call_secs(args.timeout, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
        run_repl(&toolbox, &args, timeout_secs, &mcp).await
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

struct Bridge {
    exec_id: u64,
    dir: PathBuf,
    task: tokio::task::JoinHandle<()>,
    cmd: Command,
}

fn python_bin() -> String {
    std::env::var("EL_PYTHON")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "python3".to_string())
}

/// Writes the stub, starts the socket server and builds the background python command. The caller
/// spawns it and keeps the bridge alive for as long as the job runs.
fn prepare_bridge(
    toolbox: &Toolbox,
    dir: &Path,
    code: &str,
    mcp: &[McpTool],
    exit_path: &Path,
) -> ToolResult<Bridge> {
    let (state, exec_path) = BridgeState::create(toolbox.spill_dir(), code, true)?;
    let exec_id = state.exec_id;
    let sock_path = dir.join("sock");
    let mcp_json = serde_json::to_string(&mcp_stub(mcp)).unwrap_or_else(|_| "{}".to_string());
    let stub = build_tools_py(&sock_path, &mcp_json, &exec_path);
    fs::write(dir.join("tools.py"), stub)?;

    let listener = UnixListener::bind(&sock_path)?;
    let active = Arc::new(std::sync::Mutex::new(Some(Arc::new(state))));
    // A background job outlives the call, so its listener must not keep the REPL reachable.
    let tb_for_bridge = Arc::new(toolbox.without_repl());
    let active_for_bridge = Arc::clone(&active);
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let tb = Arc::clone(&tb_for_bridge);
            let active = Arc::clone(&active_for_bridge);
            tokio::spawn(async move {
                serve_conn(tb, active, stream).await;
            });
        }
    });

    let py = python_bin();
    let prelude = toolbox.env_prelude();
    // A background job writes its own exit sidecar, so it cannot `exec` the interpreter away.
    let tmp = crate::background::shell_quote(&crate::background::exit_tmp_path(exit_path));
    let exit = crate::background::shell_quote(exit_path);
    let script = format!(
        "{prelude}\"$@\" 2>&1; __el_rc=$?; printf '%d\\n' \"$__el_rc\" > {tmp} && mv -f {tmp} {exit}; exit \"$__el_rc\""
    );
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(script)
        .arg("sh")
        .arg(&py)
        .arg("-I")
        // Unbuffered, so the job log shows what the script printed so far.
        .arg("-u")
        .arg("-c")
        .arg(LAUNCHER)
        .arg(dir)
        .arg(&exec_path)
        .current_dir(toolbox.root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    toolbox.apply_session_env(&mut cmd);
    #[cfg(unix)]
    cmd.process_group(0);

    Ok(Bridge {
        exec_id,
        dir: dir.to_path_buf(),
        task,
        cmd,
    })
}

/// Owns the socket server and its temp dir for as long as the job runs.
struct BridgeKeepalive {
    task: tokio::task::JoinHandle<()>,
    dir: PathBuf,
}

impl Drop for BridgeKeepalive {
    fn drop(&mut self) {
        self.task.abort();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Filled by the command builder once the job id, and so the exit sidecar, exist.
struct KeepaliveSlot {
    _keepalive: Arc<std::sync::Mutex<Option<BridgeKeepalive>>>,
}

async fn run_background(toolbox: &Toolbox, code: &str, mcp: &[McpTool]) -> ToolResult<String> {
    let dir = create_bridge_dir()?;
    let slot: Arc<std::sync::Mutex<Option<BridgeKeepalive>>> = Arc::default();
    let exec: Arc<std::sync::Mutex<Option<u64>>> = Arc::default();
    let exec_slot = Arc::clone(&exec);
    // shortcut: the bridge dies with the daemon, so a script adopted after a restart loses its
    // `tools` calls; give the bridge a detached server the day that matters.
    let started = toolbox
        .background()
        .spawn_with(
            Some(Box::new(KeepaliveSlot {
                _keepalive: Arc::clone(&slot),
            })),
            {
                let dir = dir.clone();
                move |log, exit| {
                    let Bridge {
                        exec_id,
                        dir: bridge_dir,
                        task,
                        cmd,
                        ..
                    } = prepare_bridge(toolbox, &dir, code, mcp, exit)?;
                    *exec_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(exec_id);
                    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(BridgeKeepalive {
                        task,
                        dir: bridge_dir,
                    });
                    let mut cmd = cmd;
                    cmd.stdout(Stdio::from(log));
                    Ok(cmd)
                }
            },
        )
        .await;
    let started = match started {
        Ok(started) => started,
        Err(err) => {
            let _ = fs::remove_dir_all(&dir);
            return Err(err);
        }
    };
    let exec_id = exec.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or(0);
    Ok(format!("{started}[saved exec #{exec_id}]\n"))
}

async fn serve_conn(
    toolbox: Arc<Toolbox>,
    active: Arc<std::sync::Mutex<Option<Arc<BridgeState>>>>,
    stream: UnixStream,
) {
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
        // Clone the active state and drop the std guard before any await.
        let candidate = active.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let resp = match candidate {
            None => BridgeResponse {
                id: req.id,
                result: None,
                error: Some("execute bridge idle".to_string()),
            },
            Some(state) if req.exec_id != 0 && req.exec_id != state.exec_id => BridgeResponse {
                id: req.id,
                result: None,
                error: Some(format!("execute bridge closed for exec #{}", state.exec_id)),
            },
            Some(state) => serve_one(&toolbox, &state, req).await,
        };
        if write_resp(&mut writer, resp).await.is_err() {
            break;
        }
    }
}

async fn serve_one(toolbox: &Toolbox, state: &BridgeState, req: BridgeRequest) -> BridgeResponse {
    if !req.op.is_empty() {
        return match state.handle_op(&req.op, &req.args) {
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
        };
    }
    if req.tool == "execute" {
        return BridgeResponse {
            id: req.id,
            result: None,
            error: Some("nested execute is not supported inside an execute script".to_string()),
        };
    }
    let started = std::time::Instant::now();
    // A script's `read` of an image answers marker text; the picture rides the execute
    // call's own MCP result, exactly as a direct `read` hands one to the model.
    let image = (req.tool == "read")
        .then(|| crate::read::image(toolbox, &req.args))
        .flatten();
    let res = match image {
        Some(Ok((mime, data))) => {
            state
                .images
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((mime, data));
            Ok(format!("[image {mime}]"))
        }
        Some(Err(err)) => Err(err),
        None => {
            crate::IN_SCRIPT
                .scope((), toolbox.call_inner(&req.tool, &req.args))
                .await
        }
    };
    let ms = started.elapsed().as_millis() as u64;
    state.record_child(&req.tool, &req.args, ms, res.is_ok());
    match res {
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

/// The persistent worker's own protocol: one NDJSON request in, one reply line out, on the
/// duplicated control descriptors, so a script writing to fd 1 cannot corrupt the framing.
const REPL_LAUNCHER: &str = r#"import ast, builtins, json, os, sys, traceback, types

_DIR = sys.argv[1]
sys.path.insert(0, _DIR)
_CTRL_IN = os.fdopen(os.dup(0), "r", encoding="utf-8", buffering=1)
_CTRL_OUT = os.fdopen(os.dup(1), "w", encoding="utf-8", buffering=1)
os.dup2(os.open(os.devnull, os.O_RDWR), 0)
_STDOUT, _STDERR = sys.stdout, sys.stderr
import tools
_NS = {"__name__": "__main__", "tools": tools}


def _vars():
    parts = []
    for name in sorted(_NS):
        if name.startswith("_") or name == "tools":
            continue
        value = _NS[name]
        if isinstance(value, types.ModuleType) or isinstance(value, (types.BuiltinFunctionType, types.BuiltinMethodType)):
            continue
        kind = type(value).__name__
        if isinstance(value, (list, dict, set, tuple, str, bytes)):
            kind = "%s(%d)" % (kind, len(value))
        parts.append("%s:%s" % (name, kind))
        if len(parts) == 8:
            break
    text = ", ".join(parts)
    return text[:159] + "\u2026" if len(text) > 160 else text

while True:
    line = _CTRL_IN.readline()
    if not line:
        break
    req = json.loads(line)
    out_fd = os.open(req["out_path"], os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    err_fd = os.open(req["err_path"], os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    _STDOUT.flush()
    _STDERR.flush()
    saved = (os.dup(1), os.dup(2))
    os.dup2(out_fd, 1)
    os.dup2(err_fd, 2)
    os.close(out_fd)
    os.close(err_fd)
    out = os.fdopen(os.dup(1), "w", encoding="utf-8", buffering=1)
    err = os.fdopen(os.dup(2), "w", encoding="utf-8", buffering=1)
    sys.stdout, sys.stderr = out, err
    path = req["exec_path"]
    exit_code = 0
    try:
        tools._begin_call(req["exec_id"], path, req["mcp"])
        tree = ast.parse(req["code"], path, "exec")
        tail = tree.body[-1] if tree.body and isinstance(tree.body[-1], ast.Expr) else None
        if tail is not None:
            if len(tree.body) > 1:
                exec(compile(ast.Module(body=tree.body[:-1], type_ignores=[]), path, "exec"), _NS)
            value = eval(compile(ast.Expression(tail.value), path, "eval"), _NS)
        else:
            exec(compile(tree, path, "exec"), _NS)
            value = None
        if value is not None:
            shown = repr(value)
            if len(shown) > __AUTO_RETURN_CAP__:
                shown = shown[:__AUTO_RETURN_CAP__] + "\u2026 (+%d chars; use print(...) for full output)" % (len(shown) - __AUTO_RETURN_CAP__)
            print(shown)
    except SystemExit as exc:
        code = exc.code
        if code not in (None, 0):
            exit_code = code if isinstance(code, int) else 1
            if not isinstance(code, int):
                print("SystemExit: %r" % (code,), file=sys.stderr)
    except BaseException:
        traceback.print_exc()
        exit_code = 1
    finally:
        try:
            out.flush()
            err.flush()
        except Exception:
            pass
        os.dup2(saved[0], 1)
        os.dup2(saved[1], 2)
        os.close(saved[0])
        os.close(saved[1])
        out.close()
        err.close()
        sys.stdout, sys.stderr = _STDOUT, _STDERR
    _CTRL_OUT.write(json.dumps({"exec_id": req["exec_id"], "exit_code": exit_code, "vars": _vars()}) + "\n")
    _CTRL_OUT.flush()
"#;

#[derive(Deserialize)]
struct ReplReply {
    #[serde(default)]
    exec_id: u64,
    #[serde(default)]
    exit_code: i32,
    #[serde(default)]
    vars: String,
}

pub(crate) struct ReplRuntime {
    worker: AsyncMutex<Option<ReplWorker>>,
}

impl ReplRuntime {
    pub(crate) fn new() -> Self {
        Self {
            worker: AsyncMutex::new(None),
        }
    }
}

struct ReplWorker {
    pid: u32,
    dir: PathBuf,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    listener_task: tokio::task::JoinHandle<()>,
    crash: Arc<std::sync::Mutex<String>>,
    active: Arc<std::sync::Mutex<Option<Arc<BridgeState>>>>,
    turns: u64,
}

impl Drop for ReplWorker {
    fn drop(&mut self) {
        self.listener_task.abort();
        crate::bash::kill_group(self.pid);
        let _ = self.child.start_kill();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl ReplWorker {
    async fn spawn(toolbox: &Toolbox, mcp: &[McpTool]) -> ToolResult<Self> {
        let dir = repl_dir()?;
        let sock_path = dir.join("sock");
        let mcp_json = serde_json::to_string(&mcp_stub(mcp)).unwrap_or_else(|_| "{}".to_string());
        fs::write(
            dir.join("tools.py"),
            build_tools_py(&sock_path, &mcp_json, Path::new("")),
        )?;
        let listener = UnixListener::bind(&sock_path)?;
        let active: Arc<std::sync::Mutex<Option<Arc<BridgeState>>>> = Arc::default();
        let tb_for_bridge = Arc::new(toolbox.without_repl());
        let active_for_bridge = Arc::clone(&active);
        let listener_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let tb = Arc::clone(&tb_for_bridge);
                let active = Arc::clone(&active_for_bridge);
                tokio::spawn(async move {
                    serve_conn(tb, active, stream).await;
                });
            }
        });
        let launcher = REPL_LAUNCHER.replace("__AUTO_RETURN_CAP__", &AUTO_RETURN_CAP.to_string());
        // The runtime env file is sourced by the shell, as it is for `bash` and background jobs.
        let prelude = toolbox.env_prelude();
        let py = python_bin();
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("{prelude}exec \"$@\""))
            .arg("sh")
            .arg(&py)
            .arg("-I")
            .arg("-u")
            .arg("-c")
            .arg(launcher)
            .arg(&dir)
            .current_dir(toolbox.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        toolbox.apply_session_env(&mut cmd);
        #[cfg(unix)]
        {
            cmd.process_group(0);
            // SAFETY: prctl runs in the child between fork and exec.
            unsafe {
                cmd.pre_exec(|| {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::msg(format!("spawn {py}: {e}")))?;
        let pid = child.id().unwrap_or(0);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ToolError::msg("failed to open the repl control pipe"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::msg("failed to read the repl control pipe"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::msg("failed to read the repl stderr"))?;
        let crash = Arc::new(std::sync::Mutex::new(String::new()));
        let crash_for = Arc::clone(&crash);
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            *crash_for.lock().unwrap_or_else(|e| e.into_inner()) =
                String::from_utf8_lossy(&buf).to_string();
        });
        Ok(Self {
            pid,
            dir,
            child,
            stdin,
            stdout: BufReader::new(stdout),
            listener_task,
            crash,
            active,
            turns: 0,
        })
    }
}

fn repl_dir() -> ToolResult<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("el-repl-{}-{nanos}", std::process::id()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    Ok(dir)
}

struct ReplOutput {
    exec_id: u64,
    stdout: String,
    stderr: String,
    exit_code: i32,
    vars: String,
    images: Vec<Image>,
    rss: Option<u64>,
}

async fn repl_turn(
    worker: &mut ReplWorker,
    toolbox: &Toolbox,
    code: &str,
    mcp: &[McpTool],
    exec_slot: &std::sync::Mutex<Option<u64>>,
) -> ToolResult<ReplOutput> {
    let (state, exec_path) = BridgeState::create(toolbox.spill_dir(), code, false)?;
    let state = Arc::new(state);
    *exec_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(state.exec_id);
    *worker.active.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&state));
    worker.turns += 1;
    let out_path = worker.dir.join(format!("turn_{}.out", state.exec_id));
    let err_path = worker.dir.join(format!("turn_{}.err", state.exec_id));
    let request = json!({
        "exec_id": state.exec_id,
        "exec_path": exec_path.to_string_lossy(),
        "code": code,
        "mcp": mcp_stub(mcp),
        "out_path": out_path.to_string_lossy(),
        "err_path": err_path.to_string_lossy(),
    });
    let mut line = String::new();
    let written = async {
        worker
            .stdin
            .write_all(format!("{request}\n").as_bytes())
            .await?;
        worker.stdin.flush().await
    }
    .await;
    let reply = match written {
        Ok(()) => match worker.stdout.read_line(&mut line).await {
            Ok(0) => Err(ToolError::msg("the repl exited")),
            Ok(_) => serde_json::from_str::<ReplReply>(line.trim())
                .map_err(|e| ToolError::msg(format!("bad repl reply: {e}"))),
            Err(e) => Err(ToolError::Io(e)),
        },
        Err(err) => Err(ToolError::Io(err)),
    };
    *worker.active.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let result = reply.and_then(|reply| {
        if reply.exec_id == state.exec_id {
            Ok(reply)
        } else {
            Err(ToolError::msg(format!(
                "the repl answered exec #{} for exec #{}",
                reply.exec_id, state.exec_id
            )))
        }
    });
    if let Ok(reply) = &result {
        if reply.exit_code == 0 {
            state.flush();
        }
    }
    state.write_children();
    let reply = result?;
    let stdout = fs::read(&out_path).unwrap_or_default();
    let stderr = fs::read(&err_path).unwrap_or_default();
    let _ = fs::remove_file(&out_path);
    let _ = fs::remove_file(&err_path);
    Ok(ReplOutput {
        exec_id: state.exec_id,
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: String::from_utf8_lossy(&stderr).to_string(),
        exit_code: reply.exit_code,
        vars: reply.vars,
        images: state.take_images(),
        rss: repl_rss(worker.pid),
    })
}

fn repl_rss(pid: u32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        let kb = line.strip_prefix("VmRSS:")?.split_whitespace().next()?;
        kb.parse::<u64>().ok().map(|kb| kb * 1024)
    })
}

async fn take_worker(runtime: &ReplRuntime) -> Option<ReplWorker> {
    runtime.worker.lock().await.take()
}

async fn run_repl(
    toolbox: &Toolbox,
    args: &ExecuteArgs,
    timeout_secs: u64,
    mcp: &[McpTool],
) -> ToolResult<(String, Vec<Image>)> {
    let runtime = toolbox.repl().cloned().ok_or_else(|| {
        ToolError::msg("nested execute is not supported inside an execute script")
    })?;
    let exec_slot = std::sync::Mutex::new(None);
    let acquired = AtomicBool::new(false);
    let attempt = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        let mut guard = runtime.worker.lock().await;
        acquired.store(true, Ordering::SeqCst);
        if args.reset {
            *guard = None;
            if args.code.trim().is_empty() {
                return Ok(None);
            }
        }
        if guard.is_none() {
            *guard = Some(ReplWorker::spawn(toolbox, mcp).await?);
        }
        let worker = match guard.as_mut() {
            Some(worker) => worker,
            None => return Err(ToolError::msg("the repl worker vanished")),
        };
        repl_turn(worker, toolbox, &args.code, mcp, &exec_slot)
            .await
            .map(Some)
    })
    .await;
    let mut prefix = String::new();
    if args.reset {
        prefix.push_str("[repl reset]\n");
    }
    match attempt {
        Ok(Ok(None)) => Ok(("[repl reset]".to_string(), Vec::new())),
        Ok(Ok(Some(out))) => {
            let outcome = ExecOutcome::Exit(out.exit_code);
            let rss = out
                .rss
                .filter(|&rss| toolbox.repl_rss_warn_bytes().is_some_and(|cap| rss > cap));
            let vars = (!out.vars.is_empty()).then_some(out.vars.as_str());
            let text = format_output(
                toolbox,
                &args.code,
                &out.stdout,
                &out.stderr,
                outcome,
                out.exec_id,
                vars,
                rss,
            )?;
            Ok((format!("{prefix}{text}"), out.images))
        }
        Ok(Err(err)) => {
            let exec_id = exec_slot.lock().unwrap_or_else(|e| e.into_inner()).take();
            let crash = take_worker(&runtime).await.map(|worker| {
                worker
                    .crash
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            });
            let Some(exec_id) = exec_id else {
                return Err(err);
            };
            let text = format_output(
                toolbox,
                &args.code,
                "",
                crash.as_deref().unwrap_or(""),
                ExecOutcome::Exit(-1),
                exec_id,
                None,
                None,
            )?;
            Ok((format!("{prefix}{text}"), Vec::new()))
        }
        Err(_) if !acquired.load(Ordering::SeqCst) => Err(ToolError::msg(
            "another execute call is running; retry or use background: true",
        )),
        Err(_) => {
            let exec_id = exec_slot.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(worker) = take_worker(&runtime).await {
                if let Some(state) = worker
                    .active
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                {
                    state.write_children();
                }
            }
            let text = format_output(
                toolbox,
                &args.code,
                "",
                "",
                ExecOutcome::TimedOut(timeout_secs),
                exec_id.unwrap_or(0),
                None,
                None,
            )?;
            Ok((format!("{prefix}{text}"), Vec::new()))
        }
    }
}

enum ExecOutcome {
    Exit(i32),
    TimedOut(u64),
}

#[allow(clippy::too_many_arguments)]
fn format_output(
    toolbox: &Toolbox,
    code: &str,
    stdout: &str,
    stderr: &str,
    outcome: ExecOutcome,
    exec_id: u64,
    vars: Option<&str>,
    rss: Option<u64>,
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
    let mut footers = Vec::new();
    if let Some(vars) = vars.filter(|vars| !vars.is_empty()) {
        footers.push(format!("[vars: {vars}]"));
    }
    if let Some(rss) = rss {
        footers.push(format!(
            "[repl rss: {} MiB; pass reset=true to clear]",
            rss / (1024 * 1024)
        ));
    }
    footers.push(tag);
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&footers.join("\n"));
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
fn mcp_stub(tools: &[McpTool]) -> Value {
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
    json!({"specs": specs, "servers": servers, "aliases": aliases})
}

fn build_tools_py(sock_path: &Path, mcp_json: &str, exec_path: &Path) -> String {
    let sock_json = serde_json::to_string(&sock_path.to_string_lossy().to_string())
        .unwrap_or_else(|_| "\"\"".to_string());
    let exec_path_json = serde_json::to_string(&exec_path.to_string_lossy().to_string())
        .unwrap_or_else(|_| "\"\"".to_string());
    TOOLS_PY
        .replace("__SOCKET_PATH__", &sock_json)
        .replace("__EXEC_PATH__", &exec_path_json)
        .replace("__MCP__", mcp_json)
}

const TOOLS_PY: &str = r#"import json as _json
import linecache as _linecache
import socket as _socket
import sys as _sys
import threading as _threading
from concurrent.futures import ThreadPoolExecutor as _ThreadPoolExecutor

class ToolError(Exception):
    pass

_SOCKET_PATH = __SOCKET_PATH__
_EXEC_ID = 0
_EXEC_PATH = __EXEC_PATH__
_CALL_LIMIT = 500
_call_count = 0
_next_id = 0
_lock = _threading.Lock()
_tls = _threading.local()
_run_depth = 0

_BASE_SPECS = {
    "read": {"sig": "read(path, limit, offset)", "desc": "Read a file, directory, ctx: handle, URL or skill path. A goldfish reads only the Project, the skill roots and its own Session directory."},
    "grep": {"sig": "grep(pattern, case_insensitive, context, glob, max_results, output_mode, path)", "desc": "Search files with regex."},
    "edit": {"sig": "edit(input)", "desc": "Apply line-anchored hashline edits. A skill changes through its root-named el://skills path."},
    "write": {"sig": "write(path, content)", "desc": "Write a whole file, creating parent directories. ~ expands from HOME. A skill is written through its root-named el://skills path."},
    "bash": {"sig": "bash(command, background, timeout)", "desc": "Run a shell command in the project root."},
    "background": {"sig": "background(action, id, offset, timeout)", "desc": "Manage background commands."},
    "find": {"sig": "find(query, grep_keywords, limit, mode, path)", "desc": "Semantic/lexical code finder with modes."},
    "websearch": {"sig": "websearch(query, count, domains, fresh, freshness, provider)", "desc": "Search the web."},
    "spawn": {"sig": "spawn(role, text, title, tier, ticketPath, startingPoint, persona, batch)", "desc": "Start a Worker, Scout or one-shot Goldfish (persona), or start/reuse a Spec or Standards reviewer."},
    "steer": {"sig": "steer(id, text, tier, ticketPath, startingPoint)", "desc": "Continue an existing subagent Session."},
    "stop": {"sig": "stop(id)", "desc": "Stop a subagent without deleting its conversation."},
    "result": {"sig": "result(id)", "desc": "Retrieve all reports from a subagent."},
    "submit": {"sig": "submit(status, report, summary)", "desc": "Send a new report to the parent Session."},
    "retry_call": {"sig": "retry_call(id, set, patches)", "desc": "Retry failed call #id with field overrides or string patches."},
}
_MCP = __MCP__
_SPECS = dict(_BASE_SPECS)
_SPECS.update(_MCP.get("specs", {}))

def _begin_call(exec_id, exec_path, mcp):
    global _call_count, _EXEC_ID, _EXEC_PATH, _MCP, _SPECS
    with _lock:
        _call_count = 0
    _EXEC_ID = int(exec_id)
    _EXEC_PATH = exec_path or ""
    _MCP = mcp
    _SPECS = dict(_BASE_SPECS)
    _SPECS.update(_MCP.get("specs", {}))

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

def _connect():
    sock = getattr(_tls, "sock", None)
    if sock is None:
        sock = _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM)
        sock.connect(_SOCKET_PATH)
        _tls.sock = sock
        _tls.reader = sock.makefile("r")
    return sock, _tls.reader

def _reset_socket():
    reader = getattr(_tls, "reader", None)
    sock = getattr(_tls, "sock", None)
    if reader is not None:
        try:
            reader.close()
        except OSError:
            pass
    if sock is not None:
        try:
            sock.close()
        except OSError:
            pass
    _tls.sock = None
    _tls.reader = None

def _exchange(payload):
    payload["id"] = _next()
    payload["exec_id"] = _EXEC_ID
    data = (_json.dumps(payload) + "\n").encode("utf-8")
    sock, reader = _connect()
    try:
        sock.sendall(data)
    except OSError:
        # The peer may have died between calls: reconnect once, then never retry a sent request.
        _reset_socket()
        sock, reader = _connect()
        sock.sendall(data)
    try:
        line = reader.readline()
    except OSError:
        _reset_socket()
        raise ToolError("execute bridge closed")
    if not line:
        _reset_socket()
        raise ToolError("execute bridge closed")
    resp = _json.loads(line)
    if resp.get("error"):
        raise ToolError(resp["error"])
    return resp.get("result", "")

def _call(tool, args):
    _count_call()
    return _exchange({"tool": tool, "args": args})

def _op(op, args):
    return _exchange({"op": op, "args": args})

def call(tool, **kwargs):
    return _call(tool, kwargs)

def read(path=None, **kwargs):
    if path is not None: kwargs["path"] = path
    return _call("read", kwargs)

def grep(pattern=None, **kwargs):
    if pattern is not None: kwargs["pattern"] = pattern
    return _call("grep", kwargs)

def edit(input=None, **kwargs):
    if input is not None: kwargs["input"] = input
    return _call("edit", kwargs)

def write(path=None, content=None, **kwargs):
    if path is not None: kwargs["path"] = path
    if content is not None: kwargs["content"] = content
    return _call("write", kwargs)

def bash(command=None, **kwargs):
    if command is not None: kwargs["command"] = command
    return _call("bash", kwargs)

def background(action=None, **kwargs):
    if action is not None: kwargs["action"] = action
    return _call("background", kwargs)

def find(query=None, **kwargs):
    if query is not None: kwargs["query"] = query
    return _call("find", kwargs)

def websearch(query=None, **kwargs):
    if query is not None: kwargs["query"] = query
    return _call("websearch", kwargs)

def parallel(calls, max_workers=4, strict=False):
    items = [(c[0], c[1] if len(c) > 1 else {}) for c in calls]
    if not items:
        return []
    workers = max(1, min(int(max_workers or 4), 16, len(items)))
    def one(item):
        name, kwargs = item
        if name in ("edit", "write", "multiedit", "str_replace"):
            return ToolError("%s is refused inside tools.parallel: two writes can race on one file" % name)
        try:
            return _call(name, kwargs)
        except ToolError as e:
            return e
    with _ThreadPoolExecutor(max_workers=workers) as pool:
        results = list(pool.map(one, items))
    if strict:
        for result in results:
            if isinstance(result, ToolError):
                raise result
    return results

def store(key, value):
    return _op("store", {"key": key, "value": value})

def load(key, default=None):
    data = _json.loads(_op("load", {"key": key}))
    if not data.get("found"):
        return default
    return data.get("value", default)

def save(name, code, description, scope="project"):
    return _op("save", {"name": name, "code": code, "description": description, "scope": scope})

def delete(name, scope="project"):
    return _op("delete", {"name": name, "scope": scope})

def scripts():
    return _json.loads(_op("scripts", {}))

def servers():
    return _MCP["servers"]

def run(name, **args):
    global _run_depth
    if _run_depth >= 4:
        raise ToolError("saved scripts nest at most 4 deep")
    source = _json.loads(_op("script", {"name": name})).get("source", "")
    scope = {"tools": _sys.modules[__name__], "args": args, "result": None}
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
    return _json.loads(_op("exec_source", {"id": eid})).get("source", "")

def rerun(exec_id, patches=None):
    global _run_depth
    if _run_depth >= 4:
        raise ToolError("saved scripts nest at most 4 deep")
    eid = _parse_exec_id(exec_id)
    source = _json.loads(_op("exec_source", {"id": eid})).get("source", "")
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
    if _run_depth == 0:
        _op("update_exec", {"id": eid, "code": source})
        _linecache.checkcache(_EXEC_PATH)
    path = _EXEC_PATH if eid == _EXEC_ID else "<exec #%d>" % eid
    scope = {"__name__": "__main__", "tools": _sys.modules[__name__], "result": None}
    _run_depth += 1
    try:
        exec(compile(source, path, "exec"), scope)
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
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolConfig;

    async fn run(toolbox: Arc<Toolbox>, args: &Value) -> ToolResult<String> {
        run_parts(toolbox, args).await.map(|(text, _)| text)
    }

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
    async fn execute_background_keeps_the_bridge_and_the_store_alive() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "alpha\nbeta\n").unwrap();
        let tb = Arc::new(
            Toolbox::new(ToolConfig::new(dir.path(), spill.path().join("spill"))).unwrap(),
        );

        let started = run(
            Arc::clone(&tb),
            &json!({
                "code": "import time, tools\nprint(tools.read(path='notes.txt').strip())\ntools.store('bg_key', 7)\ntime.sleep(0.3)\nprint('done')",
                "background": true
            }),
        )
        .await
        .unwrap();
        assert!(started.contains("started background"), "{started}");
        assert!(started.contains("[saved exec #1]"), "{started}");
        let id = started
            .split_whitespace()
            .nth(3)
            .expect("handle")
            .to_string();

        let waited = tb
            .call("background", &json!({"action": "wait", "id": id}))
            .await
            .unwrap();
        assert!(waited.contains("exited (0)"), "{waited}");
        let log = tb
            .call("background", &json!({"action": "read", "id": id}))
            .await
            .unwrap();
        assert!(log.contains("alpha"), "{log}");
        assert!(log.contains("done"), "{log}");

        let loaded = run(
            Arc::clone(&tb),
            &json!({"code": "import tools\nprint(tools.load('bg_key'))"}),
        )
        .await
        .unwrap();
        assert!(loaded.contains("7"), "{loaded}");
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
        // #1 persisted the patched source, so a follow-up rerun builds on it.
        let saved_one = fs::read_to_string(session.path().join("exec/1.py")).unwrap();
        assert!(saved_one.contains("b = 2"), "got: {saved_one}");

        let third = run(
            Arc::clone(&tb),
            &json!({
                "code": "import tools\ntools.rerun(1, patches=[('c = 0', 'c = 5')])"
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
        assert!(inspected.contains("print(x + y)"), "got: {inspected}");
    }

    #[tokio::test]
    async fn execute_writes_inner_children_metadata_for_tool_display() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "alpha\nbeta\n").unwrap();
        let tb = Arc::new(
            Toolbox::new(ToolConfig::new(dir.path(), spill.path().join("spill"))).unwrap(),
        );

        let out = run(
            Arc::clone(&tb),
            &json!({
                "code": "import tools\ntools.read(path='notes.txt')\ntry:\n    tools.read(path='missing.txt')\nexcept tools.ToolError:\n    pass\nprint('ok')"
            }),
        )
        .await
        .unwrap();
        assert!(out.contains("[saved exec #1]"), "got: {out}");

        let children_path = spill.path().join("exec").join("1.children.json");
        let raw = fs::read_to_string(&children_path).expect("children.json written");
        let children: Vec<Value> = serde_json::from_str(&raw).unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0]["name"], "read");
        assert_eq!(children[0]["target"], "notes.txt");
        assert_eq!(children[0]["status"], "completed");
        assert_eq!(children[1]["name"], "read");
        assert_eq!(children[1]["target"], "missing.txt");
        assert_eq!(children[1]["status"], "failed");
    }

    #[test]
    fn child_target_counts_every_file_in_a_multi_file_edit_patch() {
        let single = child_target("edit", &json!({ "input": "[a.rs#1A2B]\nPUT 1.=1:\n+x\n" }));
        assert_eq!(single, "a.rs");

        let multi = child_target(
            "edit",
            &json!({
                "input": "*** Begin Patch\n[a.rs#1A2B]\nPUT 1.=1:\n+x\n\n[b.rs#3C4D]\nPUT 1.=1:\n+y\n\n[dir/./c.rs#5E6F]\nPUT 1.=1:\n+z\n*** End Patch\n"
            }),
        );
        assert_eq!(multi, "a.rs (+2 more)");
    }

    fn basic_toolbox(dir: &std::path::Path, spill: &std::path::Path) -> Arc<Toolbox> {
        Arc::new(Toolbox::new(ToolConfig::new(dir, spill.join("spill"))).unwrap())
    }

    fn reply_pid(text: &str) -> u32 {
        text.lines()
            .find_map(|line| line.trim().parse::<u32>().ok())
            .expect("the script printed its pid")
    }

    #[tokio::test]
    async fn foreground_calls_share_state_and_keep_ndjson_framing() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let first = run(
            Arc::clone(&tb),
            &json!({"code": "x = 41\nimport subprocess"}),
        )
        .await
        .unwrap();
        assert!(first.contains("[saved exec #1]"), "got: {first}");

        let second = run(
            Arc::clone(&tb),
            &json!({
                "code": "import os\nprint(x + 1)\nsubprocess.run(['echo', 'sub'])\nos.write(1, b'raw\\n')\nprint('tail')"
            }),
        )
        .await
        .unwrap();
        for needle in ["42", "sub", "raw", "tail", "[saved exec #2]"] {
            assert!(second.contains(needle), "missing {needle}: {second}");
        }
    }

    #[tokio::test]
    async fn auto_return_reports_the_tail_value_and_caps_it() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let value = run(Arc::clone(&tb), &json!({"code": "1 + 1"}))
            .await
            .unwrap();
        assert!(value.contains("2"), "got: {value}");

        let none = run(Arc::clone(&tb), &json!({"code": "None"}))
            .await
            .unwrap();
        assert_eq!(none.trim(), "[saved exec #2]");

        let long = run(Arc::clone(&tb), &json!({"code": "'x' * 3000"}))
            .await
            .unwrap();
        assert!(
            long.contains("chars; use print(...) for full output"),
            "{long}"
        );
    }

    #[tokio::test]
    async fn vars_footer_lists_only_user_globals() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let listed = run(
            Arc::clone(&tb),
            &json!({"code": "alpha = 1\n_hidden = 2\nimport os\nnames = [1] * 3"}),
        )
        .await
        .unwrap();
        assert!(listed.contains("[vars: "), "got: {listed}");
        assert!(listed.contains("alpha:int"), "got: {listed}");
        assert!(listed.contains("names:list(3)"), "got: {listed}");
        assert!(!listed.contains("_hidden"), "got: {listed}");
        assert!(!listed.contains("os:module"), "got: {listed}");

        let plain = run(
            Arc::clone(&tb),
            &json!({"reset": true, "code": "print('quiet')"}),
        )
        .await
        .unwrap();
        assert!(!plain.contains("[vars: "), "got: {plain}");
    }

    #[tokio::test]
    async fn vars_footer_caps_entries_and_chars() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let many = run(
            Arc::clone(&tb),
            &json!({"code": "a=1\nb=2\nc=3\nd=4\ne=5\nf=6\ng=7\nh=8\ni=9\nj=10"}),
        )
        .await
        .unwrap();
        let line = many
            .lines()
            .find(|line| line.starts_with("[vars: "))
            .unwrap();
        assert_eq!(line.matches(":int").count(), 8, "got: {line}");
        assert!(!line.contains("i:int") && !line.contains("j:int"), "{line}");

        let long = run(
            Arc::clone(&tb),
            &json!({"reset": true, "code": "first_variable_name = 'aaaaaaaaaaaaaaaaaaaaaaaa'\nsecond_variable_name = 'bbbbbbbbbbbbbbbbbbbb'\nthird_variable_name = 'cccccccccccccccccccc'\nfourth_variable_name = 'dddddddddddddddddddd'\nfifth_variable_name = 'eeeeeeeeeeeeeeeeeeee'\nsixth_variable_name = 'ffffffffffffffffffff'\nseventh_variable_name = 'gggggggggggggggggg'\neighth_variable_name = 'hhhhhhhhhhhhhhhhhhhh'"}),
        )
        .await
        .unwrap();
        let line = long
            .lines()
            .find(|line| line.starts_with("[vars: "))
            .unwrap();
        assert!(line.ends_with("\u{2026}]"), "got: {line}");
        assert!(line.chars().count() <= 8 + 160, "got: {line}");
    }

    #[tokio::test]
    async fn reset_matrix_clears_state_and_rejects_background() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let both = run_parts(
            Arc::clone(&tb),
            &json!({"reset": true, "background": true, "code": "print(1)"}),
        )
        .await
        .unwrap_err();
        assert!(
            both.to_string()
                .contains("reset cannot be combined with background"),
            "{both}"
        );

        run(Arc::clone(&tb), &json!({"code": "x = 9"}))
            .await
            .unwrap();
        let only_reset = run(Arc::clone(&tb), &json!({"reset": true})).await.unwrap();
        assert_eq!(only_reset, "[repl reset]");

        let fresh = run(
            Arc::clone(&tb),
            &json!({"reset": true, "code": "print('x' in dir())"}),
        )
        .await
        .unwrap();
        assert!(fresh.starts_with("[repl reset]\nFalse"), "got: {fresh}");

        let empty = run(Arc::clone(&tb), &json!({})).await.unwrap_err();
        assert!(empty.to_string().contains("code is required"), "{empty}");
    }

    #[tokio::test]
    async fn nested_execute_is_refused_inside_a_script() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let refused = crate::IN_SCRIPT
            .scope((), run_parts(Arc::clone(&tb), &json!({"code": "print(1)"})))
            .await
            .unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("nested execute is not supported inside an execute script"),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn a_busy_repl_times_out_instead_of_queueing() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());
        let runtime = tb.repl().cloned().unwrap();
        let guard = runtime.worker.lock().await;

        let out = run(Arc::clone(&tb), &json!({"code": "print(1)", "timeout": 1}))
            .await
            .unwrap_err();
        assert!(
            out.to_string()
                .contains("another execute call is running; retry or use background: true"),
            "{out}"
        );
        drop(guard);
    }

    #[tokio::test]
    async fn a_timeout_tears_down_the_worker_and_the_next_call_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let slow = run(
            Arc::clone(&tb),
            &json!({"code": "import time, tools\ntools.read('a.txt')\ntime.sleep(30)", "timeout": 1}),
        )
        .await
        .unwrap();
        let children = spill.path().join("exec/1.children.json");
        assert!(children.exists(), "missing {}", children.display());
        assert!(
            slow.contains("[timed out after 1s; saved exec #1]"),
            "{slow}"
        );

        let recovered = run(Arc::clone(&tb), &json!({"code": "print(1 + 1)"}))
            .await
            .unwrap();
        assert!(recovered.contains('2'), "{recovered}");
        assert!(recovered.contains("[saved exec #2]"), "{recovered}");
    }

    #[tokio::test]
    async fn os_exit_reports_minus_one_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let died = run(Arc::clone(&tb), &json!({"code": "import os\nos._exit(1)"}))
            .await
            .unwrap();
        assert!(died.contains("[exit status -1; saved exec #1]"), "{died}");
        assert!(!died.contains("[vars: "), "got: {died}");

        let recovered = run(Arc::clone(&tb), &json!({"code": "print('back')"}))
            .await
            .unwrap();
        assert!(recovered.contains("back"), "{recovered}");
    }

    #[tokio::test]
    async fn sys_exit_keeps_the_worker_and_its_state() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let exited = run(
            Arc::clone(&tb),
            &json!({"code": "x = 5\nimport sys\nsys.exit(2)"}),
        )
        .await
        .unwrap();
        assert!(
            exited.contains("[exit status 2; saved exec #1]"),
            "{exited}"
        );

        let kept = run(Arc::clone(&tb), &json!({"code": "print(x)"}))
            .await
            .unwrap();
        assert!(kept.contains('5'), "{kept}");
    }

    #[tokio::test]
    async fn rss_warning_is_report_only_and_optional() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = Arc::new(
            Toolbox::new(ToolConfig {
                repl_rss_warn_bytes: Some(1),
                ..ToolConfig::new(dir.path(), spill.path().join("spill"))
            })
            .unwrap(),
        );

        let warned = run(Arc::clone(&tb), &json!({"code": "x = 7"}))
            .await
            .unwrap();
        assert!(warned.contains("[repl rss: "), "{warned}");
        assert!(
            warned.contains("MiB; pass reset=true to clear]"),
            "{warned}"
        );
        let kept = run(Arc::clone(&tb), &json!({"code": "print(x)"}))
            .await
            .unwrap();
        assert!(kept.contains('7'), "{kept}");

        let quiet = Arc::new(
            Toolbox::new(ToolConfig {
                repl_rss_warn_bytes: None,
                ..ToolConfig::new(dir.path(), spill.path().join("spill-quiet"))
            })
            .unwrap(),
        );
        let silent = run(Arc::clone(&quiet), &json!({"code": "1"}))
            .await
            .unwrap();
        assert!(!silent.contains("[repl rss:"), "{silent}");
    }

    #[tokio::test]
    async fn dropping_the_toolbox_kills_the_repl_child() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let out = run(
            Arc::clone(&tb),
            &json!({"code": "import os\nprint(os.getpid())"}),
        )
        .await
        .unwrap();
        let pid = reply_pid(&out);
        drop(tb);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let state = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            if state.is_empty() || state.split_whitespace().nth(2) == Some("Z") {
                return;
            }
            std::thread::yield_now();
        }
        panic!("the repl child {pid} outlived its Toolbox");
    }

    #[tokio::test]
    async fn positional_wrappers_and_parallel_limits_work() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let out = run(
            Arc::clone(&tb),
            &json!({
                "code": "import inspect\nprint(tools.read('a.txt').strip())\nprint('hello' in tools.grep('hello'))\nprint(tools.bash('echo hi').strip())\ntools.write('b.txt', 'written')\nprint(open('b.txt').read().strip())\ntag = tools.read('a.txt').split('#')[1][:4]\ntools.edit(f'[a.txt#{tag}]\\nPUT 1.=1:\\n+world\\n')\nprint(open('a.txt').read().strip())\nprint('a.txt' in tools.find('hello', grep_keywords=['hello']))\nprint([list(inspect.signature(f).parameters)[0] for f in (tools.read, tools.grep, tools.edit, tools.write, tools.bash, tools.background, tools.find, tools.websearch)])\nprint(tools.parallel([], max_workers=0))\nprint(len(tools.parallel([('read', {'path': 'a.txt'})], max_workers=17)))\ntry:\n    tools.parallel([('read', {'path': 'missing'})], strict=True)\nexcept tools.ToolError as e:\n    print('strict', 'missing' in str(e))\nprint(type(tools.parallel([('write', {'path': 'c', 'content': 'x'})])[0]).__name__)"
            }),
        )
        .await
        .unwrap();
        for needle in [
            "hello",
            "True",
            "hi",
            "written",
            "world",
            "['path', 'pattern', 'input', 'path', 'command', 'action', 'query', 'query']",
            "[]",
            "1",
            "strict True",
            "ToolError",
        ] {
            assert!(out.contains(needle), "missing {needle}: {out}");
        }
    }

    #[tokio::test]
    async fn the_thread_local_socket_survives_repeated_calls() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let out = run(
            Arc::clone(&tb),
            &json!({
                "code": "fds = set()\nfor _ in range(20):\n    tools.read('a.txt')\n    fds.add(tools._tls.sock.fileno())\nprint('fds', len(fds))"
            }),
        )
        .await
        .unwrap();
        assert!(out.contains("fds 1"), "got: {out}");
    }

    #[tokio::test]
    async fn a_scout_cannot_reach_a_denied_tool_from_execute() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let tb = Arc::new(
            Toolbox::new(ToolConfig {
                subagent_role: Some("scout".into()),
                session_url: Some("http://127.0.0.1:1/api/sessions/scout".into()),
                ..ToolConfig::new(dir.path(), spill.path().join("spill"))
            })
            .unwrap(),
        );

        let out = run(
            Arc::clone(&tb),
            &json!({"code": "import tools\ntools.edit(input='[a.txt#0000]\\nPUT 1.=1:\\n+x\\n')"}),
        )
        .await
        .unwrap();
        assert!(out.contains("unavailable to scout"), "got: {out}");
    }

    #[tokio::test]
    async fn the_session_env_reaches_foreground_and_background() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let skills = tempfile::tempdir().unwrap();
        // Pin the config: `env_file` otherwise resolves through HOME, which a parallel jail test swaps.
        let mut config = crate::ToolConfig::new(dir.path(), spill.path().join("spill"));
        config.env_file = None;
        config.global_skills = Some(skills.path().to_path_buf());
        let tb = Arc::new(crate::Toolbox::new(config).unwrap());
        let session = spill.path().display().to_string();
        let global = skills.path().display().to_string();

        let foreground = run(
            Arc::clone(&tb),
            &json!({"code": "import os\nprint(os.environ['EMBERLOOP_SESSION_DIR'])\nprint(os.environ['EMBERLOOP_GLOBAL_SKILLS'])"}),
        )
        .await
        .unwrap();
        assert!(foreground.contains(&session), "{foreground}");
        assert!(foreground.contains(&global), "{foreground}");

        let started = run(
            Arc::clone(&tb),
            &json!({"code": "import os\nprint(os.environ['EMBERLOOP_SESSION_DIR'])", "background": true}),
        )
        .await
        .unwrap();
        let id = started
            .split_whitespace()
            .nth(3)
            .expect("the background reply names the job")
            .to_string();
        tb.call(
            "background",
            &json!({"action": "wait", "id": id, "timeout": 10}),
        )
        .await
        .unwrap();
        let log = tb
            .call("background", &json!({"action": "read", "id": id}))
            .await
            .unwrap();
        assert!(log.contains(&session), "{log}");
    }

    #[tokio::test]
    async fn the_bridge_rejects_idle_and_stale_exec_ids() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());
        let state = Arc::new(
            BridgeState::create(tb.spill_dir(), "print(1)", false)
                .unwrap()
                .0,
        );

        let stale = state.exec_id + 1;
        for (active, exec_id, needle) in [
            (
                Arc::new(std::sync::Mutex::new(None)),
                1u64,
                "execute bridge idle",
            ),
            (
                Arc::new(std::sync::Mutex::new(Some(state))),
                stale,
                "execute bridge closed for exec #",
            ),
        ] {
            let (client, server) = UnixStream::pair().unwrap();
            let tb_for = Arc::clone(&tb);
            let task = tokio::spawn(async move { serve_conn(tb_for, active, server).await });
            let request =
                json!({"id": 1, "exec_id": exec_id, "tool": "read", "args": {"path": "a.txt"}});
            let (reader, mut writer) = client.into_split();
            writer
                .write_all(format!("{request}\n").as_bytes())
                .await
                .unwrap();
            let mut lines = BufReader::new(reader).lines();
            let reply = lines.next_line().await.unwrap().unwrap();
            assert!(reply.contains(needle), "got: {reply}");
            drop(writer);
            drop(lines);
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_failed_turn_does_not_flush_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        let failed = run(
            Arc::clone(&tb),
            &json!({"code": "import tools\ntools.store('k', 1)\nraise SystemExit(3)"}),
        )
        .await
        .unwrap();
        assert!(
            failed.contains("[exit status 3; saved exec #1]"),
            "{failed}"
        );

        let missing = run(
            Arc::clone(&tb),
            &json!({"code": "import tools\nprint(tools.load('k'))"}),
        )
        .await
        .unwrap();
        assert!(missing.contains("None"), "{missing}");

        run(
            Arc::clone(&tb),
            &json!({"code": "import tools\ntools.store('k', 2)"}),
        )
        .await
        .unwrap();
        let found = run(
            Arc::clone(&tb),
            &json!({"code": "import tools\nprint(tools.load('k'))"}),
        )
        .await
        .unwrap();
        assert!(found.contains('2'), "{found}");
    }

    #[tokio::test]
    async fn the_call_cap_resets_between_turns() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let tb = basic_toolbox(dir.path(), spill.path());

        run(
            Arc::clone(&tb),
            &json!({"code": "import tools\ntools._call_count = 500"}),
        )
        .await
        .unwrap();
        let out = run(
            Arc::clone(&tb),
            &json!({"code": "import tools\nprint(tools.read('a.txt').strip())"}),
        )
        .await
        .unwrap();
        assert!(out.contains("hello"), "got: {out}");
    }
}
