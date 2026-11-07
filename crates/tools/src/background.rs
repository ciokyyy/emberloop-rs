use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Child;
use tokio::sync::Mutex;

use crate::{ToolError, ToolResult};

const READ_CHUNK: u64 = 64 * 1024;
const DEFAULT_WAIT_SECS: u64 = 60;
const MAX_WAIT_SECS: u64 = 600;
const FIRST_OUTPUT_WAIT: Duration = Duration::from_millis(50);

#[derive(Deserialize)]
struct BackgroundArgs {
    action: String,
    #[serde(default, alias = "pane", alias = "handle")]
    id: Option<String>,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    timeout: Option<u64>,
}

struct Entry {
    pid: u32,
    start_time: String,
    boot_id: String,
    log: PathBuf,
    child: Option<Child>,
    status: Option<String>,
    /// Loaded from the spill dir after a daemon restart; no `Child` handle.
    adopted: bool,
    /// The pid now belongs to some other process; never signal it.
    lost: bool,
}

#[derive(Deserialize, Serialize)]
struct Meta {
    pid: u32,
    #[serde(default)]
    start_time: String,
    #[serde(default)]
    boot_id: String,
    #[serde(default)]
    notice_emitted: bool,
}

pub struct BackgroundManager {
    dir: PathBuf,
    root: PathBuf,
    counter: AtomicU64,
    entries: Mutex<HashMap<String, Entry>>,
    notices: Mutex<Vec<String>>,
}

impl BackgroundManager {
    pub fn new(dir: impl Into<PathBuf>, root: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let (entries, notices) = load_adopted(&dir);
        Self {
            dir,
            root: root.into(),
            counter: AtomicU64::new(0),
            entries: Mutex::new(entries),
            notices: Mutex::new(notices),
        }
    }

    // Real waits: a short one so a quick command's first line lands in the
    // reply, and a 100 ms poll of a pid outside our process tree.
    #[allow(clippy::disallowed_methods)]
    pub async fn spawn(&self, command: &str, prelude: &str) -> ToolResult<String> {
        fs::create_dir_all(&self.dir)?;
        let id = self.next_id();
        let log = self.dir.join(format!("{id}.log"));
        let file = File::create(&log)?;
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!("{prelude}exec 2>&1\n{command}"))
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::null());
        #[cfg(unix)]
        cmd.process_group(0);
        let child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        let start_time = proc_start_time(pid).unwrap_or_default();
        let boot_id = boot_id();
        self.entries.lock().await.insert(
            id.clone(),
            Entry {
                pid,
                start_time: start_time.clone(),
                boot_id: boot_id.clone(),
                log: log.clone(),
                child: Some(child),
                status: None,
                adopted: false,
                lost: false,
            },
        );
        write_meta(
            &self.dir,
            &id,
            &Meta {
                pid,
                start_time,
                boot_id,
                notice_emitted: false,
            },
        );
        // Give a short command the chance to write its first line before replying.
        tokio::time::sleep(FIRST_OUTPUT_WAIT).await;
        let first = read_log(&log, 0)?;
        Ok(format!(
            "started background command {id} (pid {pid})\nlog: {}\n{}",
            log.display(),
            first.chunk
        ))
    }

    /// Exit notices discovered at startup. Draining the list is the exactly-once
    /// hand-off to the steer queue.
    pub async fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().await)
    }

    pub async fn run(&self, args: &Value) -> ToolResult<String> {
        let args: BackgroundArgs = serde_json::from_value(args.clone())?;
        match args.action.as_str() {
            "list" => self.list().await,
            "read" => self.read(args.id.as_deref(), args.offset.unwrap_or(0)),
            "wait" => self.wait(args.id.as_deref(), args.timeout).await,
            "stop" => self.stop(args.id.as_deref()).await,
            other => Err(ToolError::msg(format!(
                "unknown background action {other:?}; use list, read, wait or stop"
            ))),
        }
    }

    async fn list(&self) -> ToolResult<String> {
        let mut entries = self.entries.lock().await;
        if entries.is_empty() {
            return Ok("no background commands\n".to_string());
        }
        let mut ids: Vec<String> = entries.keys().cloned().collect();
        ids.sort();
        let mut out = String::new();
        for id in ids {
            if let Some(entry) = entries.get_mut(&id) {
                let status = refresh_status(entry);
                out.push_str(&format!(
                    "{id}  {status}  pid {}  {}\n",
                    entry.pid,
                    entry.log.display()
                ));
            }
        }
        Ok(out)
    }

    fn read(&self, id: Option<&str>, offset: u64) -> ToolResult<String> {
        let id = id.ok_or_else(|| ToolError::msg("background read needs an id"))?;
        let path = self.entry_path(id)?;
        let chunk = read_log(&path, offset)?;
        Ok(format!("{}\n[next offset: {}]\n", chunk.chunk, chunk.next))
    }

    #[allow(clippy::disallowed_methods)]
    async fn wait(&self, id: Option<&str>, timeout: Option<u64>) -> ToolResult<String> {
        let id = id.ok_or_else(|| ToolError::msg("background wait needs an id"))?;
        let secs = timeout.unwrap_or(DEFAULT_WAIT_SECS).clamp(1, MAX_WAIT_SECS);
        let mut entries = self.entries.lock().await;
        let entry = entries
            .get_mut(id)
            .ok_or_else(|| ToolError::msg(format!("unknown background id {id:?}")))?;
        if entry.lost {
            return Ok(format!("lost (pid {})\n", entry.pid));
        }
        if entry.adopted {
            let pid = entry.pid;
            let entry_start = entry.start_time.clone();
            let entry_boot = entry.boot_id.clone();
            let finished = tokio::time::timeout(Duration::from_secs(secs), async {
                while identity_matches(pid, &entry_start, &entry_boot) && process_alive(pid) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;
            if finished.is_err() {
                return Ok(format!("timeout after {secs}s\n"));
            }
            if !identity_matches(pid, &entry_start, &entry_boot) {
                entry.lost = true;
                entry.status = Some("lost".to_string());
                return Ok(format!("lost (pid {pid})\n"));
            }
            entry.adopted = false;
            entry.status = Some("exited (code unknown)".to_string());
            remove_meta(&self.dir, id);
            return Ok("exited (code unknown)\n".to_string());
        }
        let Some(child) = entry.child.as_mut() else {
            return Ok(format!(
                "already {}\n",
                wait_reply(entry.status.as_deref().unwrap_or("exited"))
            ));
        };
        match tokio::time::timeout(Duration::from_secs(secs), child.wait()).await {
            Ok(Ok(status)) => {
                let label = status_label(&status);
                entry.child = None;
                entry.status = Some(label.clone());
                Ok(format!("{}\n", wait_reply(&label)))
            }
            Ok(Err(e)) => Err(ToolError::msg(format!("wait {id:?}: {e}"))),
            Err(_) => Ok(format!("timeout after {secs}s\n")),
        }
    }

    async fn stop(&self, id: Option<&str>) -> ToolResult<String> {
        let id = id.ok_or_else(|| ToolError::msg("background stop needs an id"))?;
        let mut entries = self.entries.lock().await;
        let entry = entries
            .get_mut(id)
            .ok_or_else(|| ToolError::msg(format!("unknown background id {id:?}")))?;
        if entry.lost {
            return Ok(format!(
                "lost (pid {}): process identity does not match; not killed\n",
                entry.pid
            ));
        }
        if entry.adopted {
            if !identity_matches(entry.pid, &entry.start_time, &entry.boot_id) {
                entry.lost = true;
                entry.status = Some("lost".to_string());
                remove_meta(&self.dir, id);
                return Ok(format!(
                    "lost (pid {}): process identity does not match; not killed\n",
                    entry.pid
                ));
            }
            if !process_alive(entry.pid) {
                entry.adopted = false;
                entry.status = Some("exited (code unknown)".to_string());
                remove_meta(&self.dir, id);
                return Ok("already exited (code unknown)\n".to_string());
            }
            crate::bash::kill_group(entry.pid);
            entry.adopted = false;
            entry.status = Some("stopped".to_string());
            remove_meta(&self.dir, id);
            return Ok(format!("stopped (pid {})\n", entry.pid));
        }
        let Some(mut child) = entry.child.take() else {
            return Ok(format!(
                "already exited ({})\n",
                entry.status.as_deref().unwrap_or("unknown")
            ));
        };
        crate::bash::kill_group(entry.pid);
        let _ = child.wait().await;
        entry.status = Some("stopped".to_string());
        remove_meta(&self.dir, id);
        Ok(format!("stopped (pid {})\n", entry.pid))
    }

    fn entry_path(&self, id: &str) -> ToolResult<PathBuf> {
        valid_id(id)?;
        Ok(self.dir.join(format!("{id}.log")))
    }

    fn next_id(&self) -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let seq = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("bg_{millis:x}_{seq:02x}")
    }
}

struct LogChunk {
    chunk: String,
    next: u64,
}

fn read_log(path: &Path, offset: u64) -> ToolResult<LogChunk> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = offset.min(len);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.by_ref().take(READ_CHUNK).read_to_end(&mut buf)?;
    Ok(LogChunk {
        chunk: String::from_utf8_lossy(&buf).into_owned(),
        next: start + buf.len() as u64,
    })
}

fn refresh_status(entry: &mut Entry) -> String {
    if let Some(child) = entry.child.as_mut() {
        match child.try_wait() {
            Ok(Some(status)) => {
                let label = status_label(&status);
                entry.child = None;
                entry.status = Some(label.clone());
                label
            }
            Ok(None) => "running".to_string(),
            Err(e) => format!("unknown ({e})"),
        }
    } else if entry.lost {
        "lost".to_string()
    } else if entry.adopted {
        if !identity_matches(entry.pid, &entry.start_time, &entry.boot_id) {
            entry.lost = true;
            entry.status = Some("lost".to_string());
            "lost".to_string()
        } else if process_alive(entry.pid) {
            "running".to_string()
        } else {
            entry.adopted = false;
            entry.status = Some("exited (code unknown)".to_string());
            "exited (code unknown)".to_string()
        }
    } else {
        entry.status.clone().unwrap_or_else(|| "exited".to_string())
    }
}

fn status_label(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exited({code})"),
        None => "terminated".to_string(),
    }
}

fn wait_reply(status: &str) -> String {
    match status
        .strip_prefix("exited(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        Some(code) => format!("exited ({code})"),
        None => status.to_string(),
    }
}

fn valid_id(id: &str) -> ToolResult<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ToolError::msg(format!("invalid background id {id:?}")));
    }
    Ok(())
}

fn meta_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn write_meta(dir: &Path, id: &str, meta: &Meta) {
    if let Ok(text) = serde_json::to_string(meta) {
        let _ = fs::write(meta_path(dir, id), text);
    }
}

fn remove_meta(dir: &Path, id: &str) {
    let _ = fs::remove_file(meta_path(dir, id));
}

fn load_adopted(dir: &Path) -> (HashMap<String, Entry>, Vec<String>) {
    let mut entries = HashMap::new();
    let mut notices = Vec::new();
    let current_boot = boot_id();
    let Ok(items) = fs::read_dir(dir) else {
        return (entries, notices);
    };
    for item in items.flatten() {
        let path = item.path();
        let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(id) = id.strip_suffix(".json") else {
            continue;
        };
        if !id.starts_with("bg_") || valid_id(id).is_err() {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut meta) = serde_json::from_str::<Meta>(&text) else {
            continue;
        };
        let log = dir.join(format!("{id}.log"));
        if meta.pid == 0 || !log.exists() {
            continue;
        }
        let matches = meta.boot_id == current_boot
            && !meta.start_time.is_empty()
            && proc_start_time(meta.pid).as_deref() == Some(meta.start_time.as_str());
        let alive = matches && process_alive(meta.pid);
        let (adopted, lost, status) = if matches && alive {
            (true, false, None)
        } else if matches {
            if !meta.notice_emitted {
                notices.push(format!("background {id} exited (code unknown)"));
                meta.notice_emitted = true;
                write_meta(dir, id, &meta);
            }
            (false, false, Some("exited (code unknown)".to_string()))
        } else {
            (false, true, Some("lost".to_string()))
        };
        entries.insert(
            id.to_string(),
            Entry {
                pid: meta.pid,
                start_time: meta.start_time,
                boot_id: meta.boot_id,
                log,
                child: None,
                status,
                adopted,
                lost,
            },
        );
    }
    (entries, notices)
}

fn identity_matches(pid: u32, start_time: &str, boot_id_stored: &str) -> bool {
    !start_time.is_empty()
        && boot_id() == boot_id_stored
        && proc_start_time(pid).as_deref() == Some(start_time)
}

/// True while any job recorded in this spill dir is still alive. The daemon asks
/// before a turn-end reminder, so it never wakes a Session that still waits on a
/// background command.
pub fn any_running(dir: &Path) -> bool {
    let Ok(items) = fs::read_dir(dir) else {
        return false;
    };
    for item in items.flatten() {
        let path = item.path();
        let Some(id) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        if !id.starts_with("bg_") || valid_id(id).is_err() {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<Meta>(&text) else {
            continue;
        };
        if meta.pid == 0 || !dir.join(format!("{id}.log")).exists() {
            continue;
        }
        if identity_matches(meta.pid, &meta.start_time, &meta.boot_id) && process_alive(meta.pid) {
            return true;
        }
    }
    false
}

fn boot_id() -> String {
    fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// Field 22 (`starttime`) of `/proc/<pid>/stat`. The command name in field 2
/// can contain spaces and `)`, so split after its last `)`.
fn proc_start_time(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(19).map(str::to_string)
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let state = fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            let after_comm = &stat[stat.rfind(')')? + 1..];
            after_comm.split_whitespace().next().map(str::to_string)
        });
    if state.as_deref() == Some("Z") {
        return false;
    }
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(dir: &Path, pid: u32) {
        let meta = Meta {
            pid,
            start_time: proc_start_time(pid).unwrap_or_default(),
            boot_id: boot_id(),
            notice_emitted: true,
        };
        fs::write(
            dir.join("bg_test.json"),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();
        fs::write(dir.join("bg_test.log"), "").unwrap();
    }

    #[test]
    fn any_running_sees_only_a_live_job() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!any_running(dir.path()));
        record(dir.path(), 0);
        assert!(!any_running(dir.path()));
        record(dir.path(), u32::MAX - 1);
        assert!(!any_running(dir.path()));
        record(dir.path(), std::process::id());
        assert!(any_running(dir.path()));
    }
}
