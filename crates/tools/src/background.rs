use std::any::Any;
use std::collections::{HashMap, HashSet};
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
pub(crate) const MAX_WAIT_SECS: u64 = 600;
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
    /// A caller's own resources (a socket server, a temp dir), dropped when the job is reaped.
    keepalive: Option<Box<dyn Any + Send>>,
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
    pub async fn spawn(&self, command: &str, prelude: &str) -> ToolResult<String> {
        self.spawn_with(None, |log, exit| {
            let tmp = shell_quote(&exit_tmp_path(exit));
            let exit = shell_quote(exit);
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c")
                .arg(format!(
                    "__el_exit_trap() {{ __el_rc=$?; printf '%d\\n' \"$__el_rc\" > {tmp} && mv -f {tmp} {exit}; }}\ntrap __el_exit_trap EXIT\n{prelude}exec 2>&1\n{command}"
                ))
                .stdin(Stdio::null())
                .stdout(Stdio::from(log))
                .stderr(Stdio::null());
            Ok(cmd)
        })
        .await
    }

    /// Starts a process the caller built. `build` receives the job's log file, which it must send
    /// stdout to, and the exit sidecar it should write; `keepalive` lives exactly as long as the
    /// job, so `stop` ends it too.
    #[allow(clippy::disallowed_methods)]
    pub async fn spawn_with(
        &self,
        keepalive: Option<Box<dyn Any + Send>>,
        build: impl FnOnce(File, &Path) -> ToolResult<tokio::process::Command>,
    ) -> ToolResult<String> {
        fs::create_dir_all(&self.dir)?;
        let id = self.next_id();
        let log = self.dir.join(format!("{id}.log"));
        let exit = exit_path(&self.dir, &id);
        let mut cmd = build(File::create(&log)?, &exit)?;

        cmd.current_dir(&self.root);
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
                keepalive,
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
        self.run_capped(args, MAX_WAIT_SECS).await
    }

    /// `max_wait_secs` caps a `wait`: [`MAX_WAIT_SECS`], lowered to the harness cap.
    pub async fn run_capped(&self, args: &Value, max_wait_secs: u64) -> ToolResult<String> {
        let args: BackgroundArgs = serde_json::from_value(args.clone())?;
        match args.action.as_str() {
            "list" => self.list().await,
            "read" => self.read(args.id.as_deref(), args.offset.unwrap_or(0)),
            "wait" => {
                self.wait(args.id.as_deref(), args.timeout, max_wait_secs)
                    .await
            }
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
                let status = refresh_status(entry, &self.dir, &id);
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
    async fn wait(
        &self,
        id: Option<&str>,
        timeout: Option<u64>,
        max_wait_secs: u64,
    ) -> ToolResult<String> {
        let id = id.ok_or_else(|| ToolError::msg("background wait needs an id"))?;
        let secs = timeout.unwrap_or(DEFAULT_WAIT_SECS).clamp(1, max_wait_secs);
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
                remove_meta(&self.dir, id);
                return Ok(format!("lost (pid {pid})\n"));
            }
            entry.adopted = false;
            let label = exit_status_label(&self.dir, id);
            entry.status = Some(label.clone());
            remove_meta(&self.dir, id);
            return Ok(format!("{}\n", wait_reply(&label)));
        }
        let Some(child) = entry.child.as_mut() else {
            let label =
                cached_exit_label(&self.dir, id, entry.status.as_deref().unwrap_or("exited"));
            return Ok(format!("already {}\n", wait_reply(&label)));
        };
        match tokio::time::timeout(Duration::from_secs(secs), child.wait()).await {
            Ok(Ok(status)) => {
                let label = reaped_label(&status, &self.dir, id);
                entry.child = None;
                entry.keepalive = None;
                entry.status = Some(label.clone());
                mark_notice_emitted(&self.dir, id);
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
                let label = exit_status_label(&self.dir, id);
                entry.status = Some(label.clone());
                remove_meta(&self.dir, id);
                return Ok(format!("already {}\n", wait_reply(&label)));
            }
            crate::bash::kill_group(entry.pid);
            entry.adopted = false;
            entry.status = Some("stopped".to_string());
            remove_meta(&self.dir, id);
            return Ok(format!("stopped (pid {})\n", entry.pid));
        }
        let Some(mut child) = entry.child.take() else {
            let label =
                cached_exit_label(&self.dir, id, entry.status.as_deref().unwrap_or("unknown"));
            let reply = format!("already {}\n", wait_reply(&label));
            remove_meta(&self.dir, id);
            return Ok(reply);
        };
        entry.keepalive = None;
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

fn refresh_status(entry: &mut Entry, dir: &Path, id: &str) -> String {
    if let Some(child) = entry.child.as_mut() {
        match child.try_wait() {
            Ok(Some(status)) => {
                let label = reaped_label(&status, dir, id);
                entry.child = None;
                entry.keepalive = None;
                entry.status = Some(label.clone());
                mark_notice_emitted(dir, id);
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
            mark_notice_emitted(dir, id);
            "lost".to_string()
        } else if process_alive(entry.pid) {
            "running".to_string()
        } else {
            entry.adopted = false;
            let label = exit_status_label(dir, id);
            entry.status = Some(label.clone());
            label
        }
    } else {
        entry.status.clone().unwrap_or_else(|| "exited".to_string())
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

fn exit_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.exit"))
}

pub(crate) fn exit_tmp_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.tmp", path.display()))
}

fn read_exit_code(dir: &Path, id: &str) -> Option<i32> {
    fs::read_to_string(exit_path(dir, id))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn write_exit_code(dir: &Path, id: &str, code: i32) {
    let path = exit_path(dir, id);
    let tmp = exit_tmp_path(&path);
    if fs::write(&tmp, format!("{code}\n")).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

fn exit_status_label(dir: &Path, id: &str) -> String {
    match read_exit_code(dir, id) {
        Some(code) => format!("exited({code})"),
        None => "exited (code unknown)".to_string(),
    }
}

/// A status that already names a code outlives the sidecar `remove_meta` deletes.
fn cached_exit_label(dir: &Path, id: &str, status: &str) -> String {
    let coded = status
        .strip_prefix("exited(")
        .and_then(|rest| rest.strip_suffix(')'))
        .is_some_and(|code| code.parse::<i32>().is_ok());
    if coded {
        return status.to_string();
    }
    if status.starts_with("exited") {
        return exit_status_label(dir, id);
    }
    status.to_string()
}

/// Label a reaped owned child, recording its code when the shell wrote no sidecar.
fn reaped_label(status: &std::process::ExitStatus, dir: &Path, id: &str) -> String {
    match status.code() {
        Some(code) => {
            if read_exit_code(dir, id).is_none() {
                write_exit_code(dir, id, code);
            }
            exit_status_label(dir, id)
        }
        None => "terminated".to_string(),
    }
}

pub(crate) fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
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
    let _ = fs::remove_file(exit_path(dir, id));
}

fn mark_notice_emitted(dir: &Path, id: &str) {
    let path = meta_path(dir, id);
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut meta) = serde_json::from_str::<Meta>(&text) else {
        return;
    };
    if !meta.notice_emitted {
        meta.notice_emitted = true;
        write_meta(dir, id, &meta);
    }
}

fn exit_notice(dir: &Path, id: &str, log: &Path) -> String {
    let label = match read_exit_code(dir, id) {
        Some(code) => format!("exited ({code})"),
        None => "exited (code unknown)".to_string(),
    };
    format!("background {id} {label}\nlog: {}", log.display())
}

fn read_meta_entries(dir: &Path) -> Vec<(String, Meta, PathBuf)> {
    let Ok(items) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
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
        let log = dir.join(format!("{id}.log"));
        if meta.pid == 0 || !log.exists() {
            continue;
        }
        found.push((id.to_string(), meta, log));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// Collects exit notices for background jobs in `dir` once. `watched` lists job IDs
/// already seen running in this daemon process, so a job reaped by PID 1 still notifies.
pub fn collect_notices_for(dir: &Path, watched: &HashSet<String>) -> Vec<String> {
    let mut notices = Vec::new();
    for (id, mut meta, log) in read_meta_entries(dir) {
        if meta.notice_emitted {
            continue;
        }
        let matches = identity_matches(meta.pid, &meta.start_time, &meta.boot_id);
        let alive = matches && process_alive(meta.pid);
        if (matches && !alive) || (!matches && watched.contains(&id)) {
            notices.push(exit_notice(dir, &id, &log));
            meta.notice_emitted = true;
            write_meta(dir, &id, &meta);
        }
    }
    notices
}

/// Collects exit notices for exited (zombie) background jobs in `dir` once.
pub fn collect_notices(dir: &Path) -> Vec<String> {
    collect_notices_for(dir, &HashSet::new())
}

fn load_adopted(dir: &Path) -> (HashMap<String, Entry>, Vec<String>) {
    let mut entries = HashMap::new();
    let mut notices = Vec::new();
    for (id, mut meta, log) in read_meta_entries(dir) {
        let matches = identity_matches(meta.pid, &meta.start_time, &meta.boot_id);
        let alive = matches && process_alive(meta.pid);
        let (adopted, lost, status) = if matches && alive {
            (true, false, None)
        } else if matches {
            if !meta.notice_emitted {
                notices.push(exit_notice(dir, &id, &log));
                meta.notice_emitted = true;
                write_meta(dir, &id, &meta);
            }
            (false, false, Some(exit_status_label(dir, &id)))
        } else {
            if !meta.notice_emitted {
                meta.notice_emitted = true;
                write_meta(dir, &id, &meta);
            }
            (false, true, Some("lost".to_string()))
        };
        entries.insert(
            id,
            Entry {
                pid: meta.pid,
                start_time: meta.start_time,
                boot_id: meta.boot_id,
                log,
                child: None,
                status,
                adopted,
                lost,
                keepalive: None,
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
pub fn running_ids(dir: &Path) -> Vec<String> {
    read_meta_entries(dir)
        .into_iter()
        .filter(|(_, meta, _)| {
            identity_matches(meta.pid, &meta.start_time, &meta.boot_id) && process_alive(meta.pid)
        })
        .map(|(id, _, _)| id)
        .collect()
}

pub fn any_running(dir: &Path) -> bool {
    !running_ids(dir).is_empty()
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

    fn manager(dir: &Path) -> BackgroundManager {
        let spill = dir.join("spill");
        fs::create_dir_all(&spill).unwrap();
        BackgroundManager::new(&spill, dir)
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn id_of(reply: &str) -> String {
        reply.split_whitespace().nth(3).unwrap().to_string()
    }

    fn wait_for_sidecar(dir: &Path, id: &str) {
        let path = dir.join("spill").join(format!("{id}.exit"));
        for _ in 0..10_000 {
            if path.exists() {
                return;
            }
            std::thread::yield_now();
        }
        panic!("{id} never wrote an exit sidecar");
    }

    #[test]
    fn an_adopted_job_lists_its_recorded_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let owner = manager(dir.path());
        let started = block_on(owner.spawn("exit 3", "")).unwrap();
        let id = id_of(&started);
        wait_for_sidecar(dir.path(), &id);
        wait_for_exit(pid_of(&started));
        let list =
            block_on(manager(dir.path()).run(&serde_json::json!({"action": "list"}))).unwrap();
        assert!(list.contains("exited(3)"), "{list}");
    }

    #[test]
    fn a_completion_notice_names_the_recorded_code() {
        let dir = tempfile::tempdir().unwrap();
        let owner = manager(dir.path());
        let started = block_on(owner.spawn("exit 5", "")).unwrap();
        let id = id_of(&started);
        wait_for_sidecar(dir.path(), &id);
        wait_for_exit(pid_of(&started));
        let notices = collect_notices(&dir.path().join("spill"));
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains(&format!("{id} exited (5)")),
            "{notices:?}"
        );
    }

    #[test]
    fn a_sidecar_survives_the_notice_and_dies_with_the_meta() {
        let dir = tempfile::tempdir().unwrap();
        let owner = manager(dir.path());
        let started = block_on(owner.spawn("exit 5", "")).unwrap();
        let id = id_of(&started);
        wait_for_sidecar(dir.path(), &id);
        wait_for_exit(pid_of(&started));
        let spill = dir.path().join("spill");
        assert_eq!(collect_notices(&spill).len(), 1);
        assert!(spill.join(format!("{id}.exit")).exists());
        remove_meta(&spill, &id);
        assert!(!spill.join(format!("{id}.exit")).exists());
    }

    fn pid_of(reply: &str) -> u32 {
        reply
            .split("pid ")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .and_then(|pid| pid.parse().ok())
            .unwrap()
    }

    fn wait_for_exit(pid: u32) {
        for _ in 0..10_000 {
            if !process_alive(pid) {
                return;
            }
            std::thread::yield_now();
        }
        panic!("{pid} never exited");
    }

    #[test]
    fn a_job_without_a_sidecar_falls_back_to_code_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let spill = dir.path().join("spill");
        let owner = manager(dir.path());
        let started = block_on(owner.spawn("exit 0", "")).unwrap();
        let id = id_of(&started);
        wait_for_exit(pid_of(&started));
        let _ = fs::remove_file(spill.join(format!("{id}.exit")));
        let list =
            block_on(manager(dir.path()).run(&serde_json::json!({"action": "list"}))).unwrap();
        assert!(list.contains("exited (code unknown)"), "{list}");
    }

    #[test]
    fn wait_and_stop_read_the_code_before_remove_meta() {
        let dir = tempfile::tempdir().unwrap();
        let spill = dir.path().join("spill");
        let owner = manager(dir.path());
        let started = block_on(owner.spawn("exit 4", "")).unwrap();
        let id = id_of(&started);
        wait_for_sidecar(dir.path(), &id);
        wait_for_exit(pid_of(&started));
        let reply =
            block_on(manager(dir.path()).run(&serde_json::json!({"action": "stop", "id": id})))
                .unwrap();
        assert_eq!(reply, "already exited (4)\n");
        assert!(!spill.join(format!("{id}.json")).exists());
        assert!(!spill.join(format!("{id}.exit")).exists());
    }

    #[test]
    fn a_repeated_wait_keeps_the_code_after_stop_removed_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let owner = manager(dir.path());
        let started = block_on(owner.spawn("exit 4", "")).unwrap();
        let id = id_of(&started);
        wait_for_sidecar(dir.path(), &id);
        wait_for_exit(pid_of(&started));
        let mgr = manager(dir.path());
        let first = block_on(mgr.run(&serde_json::json!({"action": "wait", "id": id}))).unwrap();
        assert_eq!(first, "already exited (4)\n");
        let stopped = block_on(mgr.run(&serde_json::json!({"action": "stop", "id": id}))).unwrap();
        assert_eq!(stopped, "already exited (4)\n");
        let again = block_on(mgr.run(&serde_json::json!({"action": "wait", "id": id}))).unwrap();
        assert_eq!(again, "already exited (4)\n");
    }

    #[test]
    fn a_failing_script_records_the_code_set_e_produced() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = manager(dir.path());
        let started = block_on(mgr.spawn("set -e\ncat <<'EOF'\nhello\nEOF\nfalse\n", "")).unwrap();
        let id = id_of(&started);
        let waited =
            block_on(mgr.run(&serde_json::json!({"action": "wait", "id": id, "timeout": 10})))
                .unwrap();
        assert_eq!(waited, "exited (1)\n");
    }
}

#[test]
fn collect_notices_emits_once_and_skips_unwatched_stale_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let stale = Meta {
        pid: u32::MAX - 1,
        start_time: "123".to_string(),
        boot_id: boot_id(),
        notice_emitted: false,
    };
    fs::write(
        dir.path().join("bg_stale.json"),
        serde_json::to_string(&stale).unwrap(),
    )
    .unwrap();
    fs::write(dir.path().join("bg_stale.log"), "").unwrap();

    // Unwatched dead/reaped job never fires spuriously.
    assert!(collect_notices(dir.path()).is_empty());

    // Watched job whose PID already vanished (e.g. reaped by PID 1) emits once.
    let watched: HashSet<String> = ["bg_stale".to_string()].into_iter().collect();
    let notices = collect_notices_for(dir.path(), &watched);
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("bg_stale exited (code unknown)"));
    assert!(notices[0].contains("bg_stale.log"));
    assert!(collect_notices_for(dir.path(), &watched).is_empty());
}

#[test]
fn collect_notices_catches_zombie_while_parent_holds_child() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    let start_time = proc_start_time(pid).unwrap();
    for _ in 0..10_000 {
        if !process_alive(pid) {
            break;
        }
        std::thread::yield_now();
    }
    let meta = Meta {
        pid,
        start_time,
        boot_id: boot_id(),
        notice_emitted: false,
    };
    fs::write(
        dir.path().join("bg_z.json"),
        serde_json::to_string(&meta).unwrap(),
    )
    .unwrap();
    fs::write(dir.path().join("bg_z.log"), "done\n").unwrap();

    let notices = collect_notices(dir.path());
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("bg_z exited (code unknown)"));
    assert!(collect_notices(dir.path()).is_empty());
    let _ = child.wait();
}
