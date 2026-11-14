//! System health and monthly token telemetry endpoints backing the Status page.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use emberloop_core::events::{EventKind, SessionEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use store::{Project, SessionMeta, SessionState};
use utoipa::ToSchema;

use crate::models::ModelEntry;
use crate::state::AppState;

const STORAGE_CACHE_TTL: Duration = Duration::from_secs(30);
const CPU_MIN_GAP: Duration = Duration::from_millis(500);
const MAX_INFERRED_CALL_MS: u64 = 600_000;
const DAY_SECS: i64 = 86_400;

#[derive(Debug, Deserialize, Default)]
pub struct SystemStatusQuery {
    #[serde(default)]
    pub refresh: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct TokenStatusQuery {
    pub month: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HostInfoDto {
    pub hostname: String,
    pub os: String,
    pub version: String,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CpuStatsDto {
    pub percent: f64,
    pub cores: usize,
    pub load_avg_1: f64,
    pub load_avg_5: f64,
    pub load_avg_15: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatsDto {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub percent: f64,
    pub cached_bytes: u64,
    pub available_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiskStatsDto {
    pub path: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSummaryDto {
    pub active_sessions: usize,
    pub working_sessions: usize,
    pub archived_sessions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRowDto {
    pub pid: u32,
    pub name: String,
    pub detail: String,
    pub cpu_percent: f64,
    pub rss_bytes: u64,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StorageSegmentDto {
    pub id: String,
    pub label: String,
    pub bytes: u64,
    pub percent: f64,
    pub tone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StorageBreakdownDto {
    pub root_path: String,
    pub total_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_percent: f64,
    pub segments: Vec<StorageSegmentDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatusDto {
    pub sampled_at: u64,
    pub host: HostInfoDto,
    pub cpu: CpuStatsDto,
    pub memory: MemoryStatsDto,
    pub disk: DiskStatsDto,
    pub runtime: RuntimeSummaryDto,
    pub processes: Vec<ProcessRowDto>,
    pub storage: StorageBreakdownDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenSummaryDto {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_hit_percent: f64,
    pub total_calls: u64,
    pub avg_latency_ms: u64,
    pub p95_latency_ms: u64,
    pub top_sink_name: String,
    pub top_sink_tokens: u64,
    pub top_sink_share_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenTelemetryRowDto {
    pub id: String,
    pub model: String,
    pub provider: String,
    pub harness: String,
    pub harness_label: String,
    pub project_id: String,
    pub project_name: String,
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub cache_hit_percent: f64,
    pub total_latency_ms: u64,
    pub avg_latency_ms: u64,
    pub p95_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenSinkDto {
    pub name: String,
    pub category: String,
    pub tokens: u64,
    pub share_percent: f64,
    pub calls: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenStatusDto {
    pub month: String,
    pub prev_month: String,
    pub next_month: String,
    pub available_months: Vec<String>,
    pub summary: TokenSummaryDto,
    pub rows: Vec<TokenTelemetryRowDto>,
    pub sinks: Vec<TokenSinkDto>,
}

#[derive(Clone, Copy)]
struct CpuTotals {
    total: u64,
    idle: u64,
    sampled_at: Instant,
    last_percent: f64,
}

#[derive(Clone, Copy)]
struct ProcCpuSample {
    jiffies: u64,
    host_total: u64,
    last_percent: f64,
}

struct CachedStorage {
    at: Instant,
    root: String,
    data: StorageBreakdownDto,
}

fn cpu_cache() -> &'static Mutex<Option<CpuTotals>> {
    static CACHE: OnceLock<Mutex<Option<CpuTotals>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn proc_cpu_cache() -> &'static Mutex<HashMap<(u32, u64), ProcCpuSample>> {
    static CACHE: OnceLock<Mutex<HashMap<(u32, u64), ProcCpuSample>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn storage_cache() -> &'static Mutex<Option<CachedStorage>> {
    static CACHE: OnceLock<Mutex<Option<CachedStorage>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

#[utoipa::path(
    get,
    path = "/api/status/system",
    params(
        ("refresh" = Option<bool>, Query, description = "Bypass the short storage walk cache")
    ),
    responses(
        (status = 200, description = "Host, runtime process, and data root storage health", body = SystemStatusDto)
    )
)]
pub async fn get_system_status(
    State(state): State<AppState>,
    Query(query): Query<SystemStatusQuery>,
) -> Json<SystemStatusDto> {
    let runtime = summarize_runtime(&state).await;
    let root = state.data_root.root().to_path_buf();
    let force_refresh = query.refresh;

    let status =
        tokio::task::spawn_blocking(move || build_system_status(&root, runtime, force_refresh))
            .await
            .unwrap_or_else(|_| empty_system_status(state.data_root.root()));

    Json(status)
}

#[utoipa::path(
    get,
    path = "/api/status/tokens",
    params(
        ("month" = Option<String>, Query, description = "UTC month in YYYY-MM format (defaults to current month)")
    ),
    responses(
        (status = 200, description = "Monthly token usage, latency, and sink telemetry", body = TokenStatusDto),
        (status = 400, description = "Invalid month parameter")
    )
)]
pub async fn get_token_status(
    State(state): State<AppState>,
    Query(query): Query<TokenStatusQuery>,
) -> Result<Json<TokenStatusDto>, (StatusCode, String)> {
    let now_ms = current_time_ms();
    let (year, month) = match query.month.as_deref() {
        Some(raw) => parse_month(raw).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("Invalid month '{raw}', expected YYYY-MM"),
            )
        })?,
        None => year_month_from_ms(now_ms),
    };

    let sessions = state.sessions.list(None).await;
    let projects = state.projects.list().await;
    let models = state.models.entries().await;
    let data_root = state.data_root.clone();

    let dto = tokio::task::spawn_blocking(move || {
        aggregate_tokens(
            &data_root,
            &sessions,
            &projects,
            &models,
            (year, month),
            now_ms,
        )
    })
    .await
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(Json(dto))
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn summarize_runtime(state: &AppState) -> RuntimeSummaryDto {
    let sessions = state.sessions.list(None).await;
    let mut active_sessions = 0usize;
    let mut working_sessions = 0usize;
    let mut archived_sessions = 0usize;

    for meta in &sessions {
        if meta.state == SessionState::Archived {
            archived_sessions += 1;
            continue;
        }
        active_sessions += 1;
        if let Ok(log) = state.get_event_log(&meta.id).await {
            if let Ok(Some(ev)) = log.latest(&EventKind::Status).await {
                if ev.data.get("state").and_then(Value::as_str) == Some("working") {
                    working_sessions += 1;
                }
            }
        }
    }

    RuntimeSummaryDto {
        active_sessions,
        working_sessions,
        archived_sessions,
    }
}

fn sample_cpu_percent(cores: usize, load_avg_1: f64) -> f64 {
    let Ok(mut guard) = cpu_cache().lock() else {
        return 0.0;
    };
    if let Some(prev) = *guard {
        if prev.sampled_at.elapsed() < CPU_MIN_GAP {
            return prev.last_percent;
        }
    }
    let Some((total, idle)) = read_cpu_totals() else {
        return 0.0;
    };
    let pct = match *guard {
        Some(p) => {
            let dt = total.saturating_sub(p.total);
            let di = idle.saturating_sub(p.idle);
            if dt == 0 {
                p.last_percent
            } else {
                ((dt.saturating_sub(di)) as f64 / dt as f64 * 100.0).clamp(0.0, 100.0)
            }
        }
        None => (load_avg_1 / cores.max(1) as f64 * 100.0).clamp(0.0, 100.0),
    };
    *guard = Some(CpuTotals {
        total,
        idle,
        sampled_at: Instant::now(),
        last_percent: pct,
    });
    pct
}

fn read_cpu_totals() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    let nums: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|s| s.parse::<u64>().ok())
        .collect();
    if nums.len() < 4 {
        return None;
    }
    let total: u64 = nums.iter().sum();
    let idle = nums[3].saturating_add(*nums.get(4).unwrap_or(&0));
    Some((total, idle))
}

fn build_system_status(
    root: &Path,
    runtime: RuntimeSummaryDto,
    force_refresh: bool,
) -> SystemStatusDto {
    let version = option_env!("EMBERLOOP_BUILD_VERSION")
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_string();
    let cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    let (load_avg_1, load_avg_5, load_avg_15) = read_load_avg();
    let cpu_percent = sample_cpu_percent(cores, load_avg_1);
    let memory = read_memory_stats();
    let disk = read_disk_stats(root);
    let storage = compute_storage(root, disk.total_bytes, force_refresh);
    let processes = collect_processes(&version, &runtime);

    SystemStatusDto {
        sampled_at: current_time_ms(),
        host: HostInfoDto {
            hostname: read_hostname(),
            os: read_os_label(),
            version,
            uptime_seconds: read_uptime_secs(),
        },
        cpu: CpuStatsDto {
            percent: round1(cpu_percent),
            cores,
            load_avg_1,
            load_avg_5,
            load_avg_15,
        },
        memory,
        disk,
        runtime,
        processes,
        storage,
    }
}

fn empty_system_status(root: &Path) -> SystemStatusDto {
    SystemStatusDto {
        sampled_at: current_time_ms(),
        host: HostInfoDto {
            hostname: "localhost".into(),
            os: std::env::consts::OS.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            uptime_seconds: 0,
        },
        cpu: CpuStatsDto {
            percent: 0.0,
            cores: 1,
            load_avg_1: 0.0,
            load_avg_5: 0.0,
            load_avg_15: 0.0,
        },
        memory: MemoryStatsDto {
            used_bytes: 0,
            total_bytes: 0,
            percent: 0.0,
            cached_bytes: 0,
            available_bytes: 0,
            swap_used_bytes: 0,
            swap_total_bytes: 0,
        },
        disk: DiskStatsDto {
            path: root.to_string_lossy().to_string(),
            used_bytes: 0,
            total_bytes: 0,
            available_bytes: 0,
            percent: 0.0,
        },
        runtime: RuntimeSummaryDto {
            active_sessions: 0,
            working_sessions: 0,
            archived_sessions: 0,
        },
        processes: Vec::new(),
        storage: StorageBreakdownDto {
            root_path: root.to_string_lossy().to_string(),
            total_used_bytes: 0,
            disk_total_bytes: 0,
            disk_percent: 0.0,
            segments: Vec::new(),
        },
    }
}

fn read_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

fn read_os_label() -> String {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().split('-').next().unwrap_or("").to_string())
        .unwrap_or_default();
    if release.is_empty() {
        "Linux".into()
    } else {
        format!("Linux {release}")
    }
}

fn read_uptime_secs() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .map(|v| v.max(0.0) as u64)
        .unwrap_or(0)
}

fn read_load_avg() -> (f64, f64, f64) {
    let Ok(text) = std::fs::read_to_string("/proc/loadavg") else {
        return (0.0, 0.0, 0.0);
    };
    let mut parts = text.split_whitespace();
    let l1 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l5 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l15 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    (round2(l1), round2(l5), round2(l15))
}

fn read_memory_stats() -> MemoryStatsDto {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut vals: HashMap<&str, u64> = HashMap::new();
    for line in text.lines() {
        if let Some((k, rest)) = line.split_once(':') {
            if let Some(kb) = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok())
            {
                vals.insert(k.trim(), kb.saturating_mul(1024));
            }
        }
    }
    let total = vals.get("MemTotal").copied().unwrap_or(0);
    let available = vals.get("MemAvailable").copied().unwrap_or(0);
    let used = total.saturating_sub(available);
    let cached = vals
        .get("Cached")
        .copied()
        .unwrap_or(0)
        .saturating_add(vals.get("Buffers").copied().unwrap_or(0));
    let swap_total = vals.get("SwapTotal").copied().unwrap_or(0);
    let swap_free = vals.get("SwapFree").copied().unwrap_or(0);
    let swap_used = swap_total.saturating_sub(swap_free);
    let percent = if total > 0 {
        round1(used as f64 / total as f64 * 100.0)
    } else {
        0.0
    };
    MemoryStatsDto {
        used_bytes: used,
        total_bytes: total,
        percent,
        cached_bytes: cached,
        available_bytes: available,
        swap_used_bytes: swap_used,
        swap_total_bytes: swap_total,
    }
}

fn read_disk_stats(path: &Path) -> DiskStatsDto {
    let target = existing_ancestor(path);
    let Ok(c_path) = std::ffi::CString::new(target.to_string_lossy().as_bytes()) else {
        return DiskStatsDto {
            path: path.to_string_lossy().to_string(),
            used_bytes: 0,
            total_bytes: 0,
            available_bytes: 0,
            percent: 0.0,
        };
    };
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return DiskStatsDto {
            path: path.to_string_lossy().to_string(),
            used_bytes: 0,
            total_bytes: 0,
            available_bytes: 0,
            percent: 0.0,
        };
    }
    let block = u64::from(stat.f_frsize as u32).max(stat.f_bsize);
    let total = stat.f_blocks.saturating_mul(block);
    let avail = stat.f_bavail.saturating_mul(block);
    let free = stat.f_bfree.saturating_mul(block);
    let used = total.saturating_sub(free);
    let percent = if total > 0 {
        round1(used as f64 / total as f64 * 100.0)
    } else {
        0.0
    };
    DiskStatsDto {
        path: path.to_string_lossy().to_string(),
        used_bytes: used,
        total_bytes: total,
        available_bytes: avail,
        percent,
    }
}

fn existing_ancestor(path: &Path) -> &Path {
    let mut cur = path;
    while !cur.exists() {
        match cur.parent() {
            Some(p) => cur = p,
            None => return Path::new("/"),
        }
    }
    cur
}

struct ProcRaw {
    pid: u32,
    ppid: u32,
    comm: String,
    cmdline: String,
    jiffies: u64,
    start_ticks: u64,
    rss_bytes: u64,
}

fn collect_processes(version: &str, runtime: &RuntimeSummaryDto) -> Vec<ProcessRowDto> {
    let self_pid = std::process::id();
    let all = scan_proc_tree();
    let descendants = collect_descendants(self_pid, &all);
    let host_total = read_cpu_totals().map(|(t, _)| t).unwrap_or(0);
    let mut rows = Vec::new();

    for proc_info in &all {
        if proc_info.pid != self_pid && !descendants.contains(&proc_info.pid) {
            continue;
        }
        let cpu = compute_proc_cpu(proc_info, host_total);
        rows.push(format_process_row(proc_info, version, cpu));
    }

    rows.sort_by(|a, b| {
        let a_self = a.pid == self_pid;
        let b_self = b.pid == self_pid;
        b_self
            .cmp(&a_self)
            .then_with(|| b.rss_bytes.cmp(&a.rss_bytes))
    });
    rows.truncate(8);

    let session_state = if runtime.working_sessions > 0 {
        "Working"
    } else {
        "Idle"
    };
    rows.push(ProcessRowDto {
        pid: 0,
        name: "sessions".into(),
        detail: format!(
            "{} active · {} working · {} archived",
            runtime.active_sessions, runtime.working_sessions, runtime.archived_sessions
        ),
        cpu_percent: 0.0,
        rss_bytes: 0,
        state: session_state.into(),
    });
    rows
}

fn scan_proc_tree() -> Vec<ProcRaw> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    let mut list = Vec::new();
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if let Some(raw) = read_proc_raw(pid, page_size) {
            list.push(raw);
        }
    }
    list
}

fn read_proc_raw(pid: u32, page_size: u64) -> Option<ProcRaw> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat[open + 1..close].to_string();
    let fields: Vec<&str> = stat[close + 2..].split_whitespace().collect();
    if fields.len() <= 21 {
        return None;
    }
    let ppid = fields[1].parse::<u32>().ok()?;
    let utime = fields[11].parse::<u64>().unwrap_or(0);
    let stime = fields[12].parse::<u64>().unwrap_or(0);
    let start_ticks = fields[19].parse::<u64>().unwrap_or(0);
    let rss_pages = fields[21].parse::<u64>().unwrap_or(0);
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .map(|bytes| {
            bytes
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    Some(ProcRaw {
        pid,
        ppid,
        comm,
        cmdline,
        jiffies: utime.saturating_add(stime),
        start_ticks,
        rss_bytes: rss_pages.saturating_mul(page_size),
    })
}

fn collect_descendants(root_pid: u32, procs: &[ProcRaw]) -> BTreeSet<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    let mut out = BTreeSet::new();
    let mut stack = vec![root_pid];
    while let Some(pid) = stack.pop() {
        if let Some(kids) = children.get(&pid) {
            for &kid in kids {
                if out.insert(kid) {
                    stack.push(kid);
                }
            }
        }
    }
    out
}

fn compute_proc_cpu(proc_info: &ProcRaw, host_total: u64) -> f64 {
    let Ok(mut guard) = proc_cpu_cache().lock() else {
        return 0.0;
    };
    let key = (proc_info.pid, proc_info.start_ticks);
    let pct = match guard.get(&key) {
        Some(prev) => {
            let dt = host_total.saturating_sub(prev.host_total);
            let dp = proc_info.jiffies.saturating_sub(prev.jiffies);
            if dt == 0 {
                prev.last_percent
            } else {
                (dp as f64 / dt as f64 * 100.0).clamp(0.0, 100.0)
            }
        }
        None => 0.0,
    };
    guard.insert(
        key,
        ProcCpuSample {
            jiffies: proc_info.jiffies,
            host_total,
            last_percent: pct,
        },
    );
    if guard.len() > 256 {
        guard.retain(|&(pid, _), _| Path::new(&format!("/proc/{pid}")).exists());
    }
    round1(pct)
}

fn format_process_row(p: &ProcRaw, version: &str, cpu_percent: f64) -> ProcessRowDto {
    let (name, detail, state) = if p.pid == std::process::id() {
        (
            "emberloop-daemon".into(),
            format!("v{version} · pid {}", p.pid),
            "Running",
        )
    } else if p.comm.contains("agy_acp") {
        ("agy-acp".into(), "Antigravity harness".into(), "Running")
    } else if p.comm.contains("localharness") {
        ("localharness".into(), "ACP tool runtime".into(), "Running")
    } else if p.comm.contains("figcraft-server") {
        (
            "figcraft-server".into(),
            "Design canvas server".into(),
            "Healthy",
        )
    } else if p.cmdline.contains("figcraft-cli") {
        (
            "figcraft-mcp".into(),
            "FigCraft MCP bridge".into(),
            "Healthy",
        )
    } else if p.cmdline.contains("emberloop-daemon") && p.cmdline.contains(" mcp") {
        (
            "emberloop-mcp".into(),
            "Session MCP bridge".into(),
            "Running",
        )
    } else if matches!(p.comm.as_str(), "claude" | "codex" | "pi") {
        (p.comm.clone(), "Harness worker".into(), "Working")
    } else {
        (p.comm.clone(), format!("pid {}", p.pid), "Running")
    };
    ProcessRowDto {
        pid: p.pid,
        name,
        detail,
        cpu_percent,
        rss_bytes: p.rss_bytes,
        state: state.into(),
    }
}

fn compute_storage(root: &Path, disk_total_bytes: u64, force_refresh: bool) -> StorageBreakdownDto {
    let root_str = root.to_string_lossy().to_string();
    if !force_refresh {
        if let Ok(guard) = storage_cache().lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.root == root_str && cached.at.elapsed() < STORAGE_CACHE_TTL {
                    return cached.data.clone();
                }
            }
        }
    }

    let mut harnesses = 0u64;
    let mut sessions = 0u64;
    let mut releases = 0u64;
    let mut other = 0u64;

    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let size = dir_size(&entry.path());
            let name = entry.file_name().to_string_lossy().to_string();
            match name.as_str() {
                "claude-homes" | "pi-homes" | "agy-acp" | "pi" => harnesses += size,
                "sessions" => sessions += size,
                "releases" | "models" | "baselines" => releases += size,
                _ => other += size,
            }
        }
    }

    let total_used = harnesses + sessions + releases + other;
    let pct = |bytes: u64| {
        if total_used > 0 {
            round1(bytes as f64 / total_used as f64 * 100.0)
        } else {
            0.0
        }
    };
    let disk_percent = if disk_total_bytes > 0 {
        round1(total_used as f64 / disk_total_bytes as f64 * 100.0)
    } else {
        0.0
    };
    let data = StorageBreakdownDto {
        root_path: root_str.clone(),
        total_used_bytes: total_used,
        disk_total_bytes,
        disk_percent,
        segments: vec![
            StorageSegmentDto {
                id: "harnesses".into(),
                label: "Harness homes".into(),
                bytes: harnesses,
                percent: pct(harnesses),
                tone: "accent".into(),
            },
            StorageSegmentDto {
                id: "sessions".into(),
                label: "Sessions & logs".into(),
                bytes: sessions,
                percent: pct(sessions),
                tone: "needs".into(),
            },
            StorageSegmentDto {
                id: "releases".into(),
                label: "Releases & models".into(),
                bytes: releases,
                percent: pct(releases),
                tone: "done".into(),
            },
            StorageSegmentDto {
                id: "other".into(),
                label: "Config & cache".into(),
                bytes: other,
                percent: pct(other),
                tone: "muted".into(),
            },
        ],
    };

    if let Ok(mut guard) = storage_cache().lock() {
        *guard = Some(CachedStorage {
            at: Instant::now(),
            root: root_str,
            data: data.clone(),
        });
    }
    data
}

fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        return 0;
    }
    if !meta.is_dir() {
        return meta.len();
    }
    let mut stack = vec![path.to_path_buf()];
    let mut total = 0u64;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(m) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if m.file_type().is_symlink() {
                continue;
            }
            if m.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(m.len());
            }
        }
    }
    total
}

#[derive(Default)]
struct RowAccum {
    model: String,
    provider: String,
    harness: String,
    harness_label: String,
    project_id: String,
    project_name: String,
    calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    cache_creation_tokens: u64,
    latencies: Vec<u64>,
}

#[derive(Default)]
struct SinkAccum {
    category: String,
    tokens: u64,
    calls: u64,
}

fn aggregate_tokens(
    data_root: &store::DataRoot,
    sessions: &[SessionMeta],
    projects: &[Project],
    models: &[ModelEntry],
    ym: (i32, u32),
    now_ms: u64,
) -> TokenStatusDto {
    let month_str = format_month(ym.0, ym.1);
    let prev_month = shift_month(ym.0, ym.1, -1);
    let next_month = shift_month(ym.0, ym.1, 1);
    let (start_ms, end_ms) = month_bounds_ms(ym.0, ym.1);

    let project_names: HashMap<&str, &str> = projects
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();
    let model_entries: HashMap<&str, &ModelEntry> =
        models.iter().map(|m| (m.id.as_str(), m)).collect();

    let mut available_months = collect_available_months(sessions, now_ms, &month_str);
    available_months.sort();
    available_months.dedup();

    let mut rows_map: HashMap<String, RowAccum> = HashMap::new();
    let mut sinks_map: HashMap<String, SinkAccum> = HashMap::new();
    let mut all_latencies: Vec<u64> = Vec::new();

    for meta in sessions {
        if meta.created_at >= end_ms {
            continue;
        }
        let last_touch = meta.updated_at.max(meta.last_used_at.unwrap_or(0));
        if last_touch.saturating_add(DAY_SECS as u64 * 1000) < start_ms {
            continue;
        }
        let lines = load_session_jsonl_lines(data_root, &meta.id);
        if lines.is_empty() {
            continue;
        }
        let (harness, harness_label, provider) =
            resolve_model_identity(&meta.model, model_entries.get(meta.model.as_str()).copied());
        let project_name = project_names
            .get(meta.project_id.as_str())
            .copied()
            .unwrap_or(&meta.project_id)
            .to_string();
        let row_key = format!("{harness}|{}|{}", meta.model, meta.project_id);

        scan_session_events(
            &lines,
            (start_ms, end_ms),
            meta,
            (&harness, &harness_label, &provider, &project_name, &row_key),
            &mut rows_map,
            &mut sinks_map,
            &mut all_latencies,
        );
    }

    build_token_status_dto(
        month_str,
        prev_month,
        next_month,
        available_months,
        rows_map,
        sinks_map,
        all_latencies,
    )
}

fn collect_available_months(
    sessions: &[SessionMeta],
    now_ms: u64,
    active_month: &str,
) -> Vec<String> {
    let mut months: BTreeSet<String> = BTreeSet::new();
    let (ny, nm) = year_month_from_ms(now_ms);
    months.insert(format_month(ny, nm));
    months.insert(active_month.to_string());
    for meta in sessions {
        let (cy, cm) = year_month_from_ms(meta.created_at);
        months.insert(format_month(cy, cm));
        let (uy, um) = year_month_from_ms(meta.updated_at);
        months.insert(format_month(uy, um));
    }
    months.into_iter().collect()
}

fn resolve_model_identity(model: &str, entry: Option<&ModelEntry>) -> (String, String, String) {
    let harness = match entry.map(|e| e.source.as_str()) {
        Some(src) if !src.is_empty() => src.to_string(),
        _ if model.starts_with("claude/") => "claude".to_string(),
        _ if model.contains('/') => "pi".to_string(),
        _ => "agy".to_string(),
    };
    let harness_label = match harness.as_str() {
        "claude" => "Claude",
        "pi" => "pi-rs",
        "agy" => "Antigravity",
        other => other,
    }
    .to_string();
    let provider = match entry.map(|e| e.provider.as_str()) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => model
            .split('/')
            .next()
            .filter(|_| model.contains('/'))
            .unwrap_or(harness.as_str())
            .to_string(),
    };
    (harness, harness_label, provider)
}

fn load_session_jsonl_lines(data_root: &store::DataRoot, session_id: &str) -> Vec<String> {
    let events_dir = data_root.session_events_dir(session_id);
    if events_dir.is_dir() {
        let mut files: Vec<_> = std::fs::read_dir(&events_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .collect();
        files.sort();
        let mut lines = Vec::new();
        for path in files {
            if let Ok(text) = std::fs::read_to_string(path) {
                lines.extend(text.lines().map(str::to_string));
            }
        }
        if !lines.is_empty() {
            return lines;
        }
    }

    let archive = data_root.session_archive_file(session_id);
    if let Ok(entries) = store::archive::read_archived_events(&archive) {
        let mut lines = Vec::new();
        for (name, bytes) in entries {
            if name.ends_with(".jsonl") {
                let text = String::from_utf8_lossy(&bytes);
                lines.extend(text.lines().map(str::to_string));
            }
        }
        return lines;
    }
    Vec::new()
}

#[allow(clippy::too_many_arguments)]
fn scan_session_events(
    lines: &[String],
    bounds: (u64, u64),
    meta: &SessionMeta,
    identity: (&str, &str, &str, &str, &str),
    rows_map: &mut HashMap<String, RowAccum>,
    sinks_map: &mut HashMap<String, SinkAccum>,
    all_latencies: &mut Vec<u64>,
) {
    let (start_ms, end_ms) = bounds;
    let (harness, harness_label, provider, project_name, row_key) = identity;
    let mut prev_marker_at: Option<u64> = None;
    let mut pending_tools: HashMap<String, (String, u64)> = HashMap::new();
    let mut round_tools: Vec<(String, u64)> = Vec::new();

    for line in lines {
        let Ok(ev) = serde_json::from_str::<SessionEvent>(line) else {
            continue;
        };
        match ev.kind {
            EventKind::Prompt | EventKind::SteerDelivered => {
                prev_marker_at = Some(ev.at);
                round_tools.clear();
            }
            EventKind::ToolStarted => {
                if let Some(id) = ev.data.get("id").and_then(Value::as_str) {
                    let clean = ev
                        .data
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(normalize_tool_name);
                    let input_w = ev
                        .data
                        .get("input")
                        .map(|v| v.to_string().len() as u64 / 4)
                        .unwrap_or(1)
                        .max(1);
                    if let Some((existing_name, existing_w)) = pending_tools.get_mut(id) {
                        if existing_name == "tool" {
                            if let Some(name) = clean {
                                *existing_name = name;
                            }
                        }
                        *existing_w = (*existing_w).max(input_w);
                    } else {
                        pending_tools.insert(
                            id.to_string(),
                            (clean.unwrap_or_else(|| "tool".into()), input_w),
                        );
                    }
                }
            }
            EventKind::ToolFinished => {
                prev_marker_at = Some(ev.at);
                let out_w = ev
                    .data
                    .get("output")
                    .and_then(Value::as_str)
                    .map(|s| s.len() as u64 / 4)
                    .unwrap_or(1)
                    .max(1);
                let tool_name = ev
                    .data
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|id| pending_tools.remove(id))
                    .map(|(name, in_w)| (name, in_w.saturating_add(out_w)))
                    .unwrap_or_else(|| ("tools".into(), out_w));
                round_tools.push(tool_name);
            }
            EventKind::Usage => {
                let latency_ms = resolve_usage_latency(&ev, prev_marker_at);
                prev_marker_at = Some(ev.at);
                if ev.at >= start_ms && ev.at < end_ms {
                    record_usage_event(
                        &ev.data,
                        latency_ms,
                        meta,
                        (harness, harness_label, provider, project_name, row_key),
                        &round_tools,
                        rows_map,
                        sinks_map,
                        all_latencies,
                    );
                }
                round_tools.clear();
            }
            _ => {}
        }
    }
}

fn resolve_usage_latency(ev: &SessionEvent, prev_marker_at: Option<u64>) -> Option<u64> {
    if let Some(ms) = ev.data.get("durationMs").and_then(Value::as_u64) {
        return Some(ms);
    }
    let prev = prev_marker_at?;
    let delta = ev.at.saturating_sub(prev);
    (delta > 0 && delta <= MAX_INFERRED_CALL_MS).then_some(delta)
}

#[allow(clippy::too_many_arguments)]
fn record_usage_event(
    data: &Value,
    latency_ms: Option<u64>,
    meta: &SessionMeta,
    identity: (&str, &str, &str, &str, &str),
    round_tools: &[(String, u64)],
    rows_map: &mut HashMap<String, RowAccum>,
    sinks_map: &mut HashMap<String, SinkAccum>,
    all_latencies: &mut Vec<u64>,
) {
    let (harness, harness_label, provider, project_name, row_key) = identity;
    let input = json_u64(data, "inputTokens");
    let output = json_u64(data, "outputTokens");
    let cached = json_u64(data, "cachedTokens");
    let cache_create = json_u64(data, "cacheCreationTokens");

    let row = rows_map
        .entry(row_key.to_string())
        .or_insert_with(|| RowAccum {
            model: meta.model.clone(),
            provider: provider.to_string(),
            harness: harness.to_string(),
            harness_label: harness_label.to_string(),
            project_id: meta.project_id.clone(),
            project_name: project_name.to_string(),
            ..RowAccum::default()
        });
    row.calls += 1;
    row.input_tokens = row.input_tokens.saturating_add(input);
    row.output_tokens = row.output_tokens.saturating_add(output);
    row.cached_tokens = row.cached_tokens.saturating_add(cached);
    row.cache_creation_tokens = row.cache_creation_tokens.saturating_add(cache_create);
    if let Some(ms) = latency_ms {
        row.latencies.push(ms);
        all_latencies.push(ms);
    }

    record_breakdown_sinks(data, output, round_tools, sinks_map);
}

fn record_breakdown_sinks(
    data: &Value,
    output_tokens: u64,
    round_tools: &[(String, u64)],
    sinks_map: &mut HashMap<String, SinkAccum>,
) {
    let Some(bd) = data.get("breakdown") else {
        add_sink(sinks_map, "Conversation", "context", output_tokens, 1);
        return;
    };
    let sys = json_u64(bd, "system").saturating_add(json_u64(bd, "toolDefinitions"));
    let conv = json_u64(bd, "conversation").saturating_add(output_tokens);
    let comp = json_u64(bd, "compression");
    let imgs = json_u64(bd, "images");
    let tools_tok = json_u64(bd, "toolCalls").saturating_add(json_u64(bd, "toolResults"));

    add_sink(sinks_map, "Context & system", "context", sys, 1);
    add_sink(sinks_map, "Conversation", "context", conv, 1);
    if comp > 0 {
        add_sink(sinks_map, "Compression", "context", comp, 1);
    }
    if imgs > 0 {
        add_sink(sinks_map, "Images", "context", imgs, 1);
    }
    if tools_tok > 0 {
        if round_tools.is_empty() {
            add_sink(sinks_map, "Tool calls & results", "tool", tools_tok, 1);
        } else {
            let weight_sum: u64 = round_tools.iter().map(|(_, w)| *w).sum::<u64>().max(1);
            for (name, w) in round_tools {
                let share = tools_tok.saturating_mul(*w) / weight_sum;
                add_sink(sinks_map, name, "tool", share, 1);
            }
        }
    }
}

fn add_sink(
    sinks: &mut HashMap<String, SinkAccum>,
    name: &str,
    category: &str,
    tokens: u64,
    calls: u64,
) {
    if tokens == 0 {
        return;
    }
    let entry = sinks.entry(name.to_string()).or_insert_with(|| SinkAccum {
        category: category.to_string(),
        tokens: 0,
        calls: 0,
    });
    entry.tokens = entry.tokens.saturating_add(tokens);
    entry.calls = entry.calls.saturating_add(calls);
}

fn build_token_status_dto(
    month: String,
    prev_month: String,
    next_month: String,
    available_months: Vec<String>,
    rows_map: HashMap<String, RowAccum>,
    sinks_map: HashMap<String, SinkAccum>,
    all_latencies: Vec<u64>,
) -> TokenStatusDto {
    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut total_cached = 0u64;
    let mut total_cache_create = 0u64;
    let mut total_calls = 0u64;

    let mut rows: Vec<TokenTelemetryRowDto> = rows_map
        .into_iter()
        .map(|(id, mut acc)| {
            total_input = total_input.saturating_add(acc.input_tokens);
            total_output = total_output.saturating_add(acc.output_tokens);
            total_cached = total_cached.saturating_add(acc.cached_tokens);
            total_cache_create = total_cache_create.saturating_add(acc.cache_creation_tokens);
            total_calls = total_calls.saturating_add(acc.calls);

            let total_tokens = acc.input_tokens.saturating_add(acc.output_tokens);
            let total_latency_ms: u64 = acc.latencies.iter().sum();
            let avg_latency_ms = total_latency_ms
                .checked_div(acc.latencies.len() as u64)
                .unwrap_or(0);
            let p95_latency_ms = percentile_p95(&mut acc.latencies);
            let cache_hit_percent = cache_hit_pct(acc.input_tokens, acc.cached_tokens);

            TokenTelemetryRowDto {
                id,
                model: acc.model,
                provider: acc.provider,
                harness: acc.harness,
                harness_label: acc.harness_label,
                project_id: acc.project_id,
                project_name: acc.project_name,
                calls: acc.calls,
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                cached_tokens: acc.cached_tokens,
                cache_creation_tokens: acc.cache_creation_tokens,
                total_tokens,
                cache_hit_percent,
                total_latency_ms,
                avg_latency_ms,
                p95_latency_ms,
            }
        })
        .collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.total_tokens));

    let sink_sum: u64 = sinks_map.values().map(|s| s.tokens).sum();
    let mut sinks: Vec<TokenSinkDto> = sinks_map
        .into_iter()
        .map(|(name, acc)| TokenSinkDto {
            name,
            category: acc.category,
            tokens: acc.tokens,
            share_percent: if sink_sum > 0 {
                round1(acc.tokens as f64 / sink_sum as f64 * 100.0)
            } else {
                0.0
            },
            calls: acc.calls,
        })
        .collect();
    sinks.sort_by_key(|a| std::cmp::Reverse(a.tokens));
    sinks.truncate(8);

    let mut lat = all_latencies;
    let avg_latency_ms = lat
        .iter()
        .sum::<u64>()
        .checked_div(lat.len() as u64)
        .unwrap_or(0);
    let p95_latency_ms = percentile_p95(&mut lat);
    let top_sink = sinks.first();

    TokenStatusDto {
        month,
        prev_month,
        next_month,
        available_months,
        summary: TokenSummaryDto {
            total_tokens: total_input.saturating_add(total_output),
            input_tokens: total_input,
            output_tokens: total_output,
            cached_tokens: total_cached,
            cache_creation_tokens: total_cache_create,
            cache_hit_percent: cache_hit_pct(total_input, total_cached),
            total_calls,
            avg_latency_ms,
            p95_latency_ms,
            top_sink_name: top_sink
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "—".into()),
            top_sink_tokens: top_sink.map(|s| s.tokens).unwrap_or(0),
            top_sink_share_percent: top_sink.map(|s| s.share_percent).unwrap_or(0.0),
        },
        rows,
        sinks,
    }
}

fn cache_hit_pct(input_tokens: u64, cached_tokens: u64) -> f64 {
    if input_tokens == 0 {
        return 0.0;
    }
    round1((cached_tokens as f64 / input_tokens as f64 * 100.0).clamp(0.0, 100.0))
}

fn percentile_p95(values: &mut [u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let idx = ((values.len() as f64) * 0.95).ceil() as usize;
    values[idx.saturating_sub(1).min(values.len() - 1)]
}

fn normalize_tool_name(raw: &str) -> String {
    let trimmed = raw.trim();
    let trimmed = trimmed
        .strip_prefix("mcp__el__")
        .or_else(|| trimmed.strip_prefix("mcp__el_"))
        .or_else(|| trimmed.strip_prefix("mcp_el_"))
        .unwrap_or(trimmed);
    let trimmed = trimmed
        .strip_prefix("mcp__")
        .and_then(|s| s.split("__").last())
        .unwrap_or(trimmed);
    let trimmed = trimmed.strip_prefix("el_").unwrap_or(trimmed).trim();
    if trimmed.is_empty() {
        "tool".to_string()
    } else {
        trimmed.to_ascii_lowercase()
    }
}

fn json_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn format_month(year: i32, month: u32) -> String {
    format!("{year:04}-{month:02}")
}

fn parse_month(raw: &str) -> Option<(i32, u32)> {
    let (y, m) = raw.split_once('-')?;
    if y.len() != 4 || m.len() != 2 {
        return None;
    }
    let year: i32 = y.parse().ok()?;
    let month: u32 = m.parse().ok()?;
    if !(1970..=2100).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    Some((year, month))
}

fn shift_month(year: i32, month: u32, delta: i32) -> String {
    let total = year * 12 + (month as i32 - 1) + delta;
    let ny = total.div_euclid(12);
    let nm = (total.rem_euclid(12) + 1) as u32;
    format_month(ny, nm)
}

fn month_bounds_ms(year: i32, month: u32) -> (u64, u64) {
    let start_days = days_from_civil(year, month, 1);
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let end_days = days_from_civil(ny, nm, 1);
    (
        (start_days * DAY_SECS).max(0) as u64 * 1000,
        (end_days * DAY_SECS).max(0) as u64 * 1000,
    )
}

fn year_month_from_ms(ms: u64) -> (i32, u32) {
    let days = (ms / 1000) as i64 / DAY_SECS;
    let (y, m, _) = civil_from_days(days);
    (y, m)
}

// Howard Hinnant's civil-from-days / days-from-civil (UTC proleptic Gregorian).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - if month <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_calendar_roundtrips_cleanly() {
        assert_eq!(parse_month("2026-11"), Some((2026, 11)));
        assert_eq!(parse_month("2026-13"), None);
        assert_eq!(shift_month(2026, 1, -1), "2025-12");
        assert_eq!(shift_month(2026, 12, 1), "2027-01");

        let (start, end) = month_bounds_ms(2026, 11);
        assert!(end > start);
        assert_eq!(year_month_from_ms(start), (2026, 11));
        assert_eq!(year_month_from_ms(end - 1), (2026, 11));
        assert_eq!(year_month_from_ms(end), (2026, 12));
    }
}
