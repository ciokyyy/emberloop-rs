//! External binary version checker backing `update --check` and the Status page.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::status::{BinariesStatusDto, BinaryChecker, ExternalBinaryDto, ExternalBinaryState};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::update;

const CACHE_FILE: &str = "binaries-cache.json";
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const REFRESH_AFTER: Duration = Duration::from_secs(30);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT_SECS: u64 = 8;

#[derive(Debug, Clone)]
struct Endpoints {
    github_api: String,
    agy_registry: String,
    claude_npm: String,
    pi: String,
    rtk: String,
    figcraft: String,
    codebase_memory_mcp: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            github_api: "https://api.github.com".into(),
            agy_registry: "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json"
                .into(),
            claude_npm: "https://registry.npmjs.org/@anthropic-ai/claude-code/latest".into(),
            pi: "yannelli/pi-rust".into(),
            rtk: "rtk-ai/rtk".into(),
            figcraft: "storytold/figcraft".into(),
            codebase_memory_mcp: "DeusData/codebase-memory-mcp".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bin {
    Agy,
    Claude,
    Pi,
    Rtk,
    Figcraft,
    CodebaseMemory,
}

struct Spec {
    id: &'static str,
    name: &'static str,
    bin: Bin,
}

const SPECS: [Spec; 6] = [
    Spec {
        id: "agy",
        name: "Antigravity ACP",
        bin: Bin::Agy,
    },
    Spec {
        id: "claude",
        name: "Claude Code",
        bin: Bin::Claude,
    },
    Spec {
        id: "pi",
        name: "pi",
        bin: Bin::Pi,
    },
    Spec {
        id: "rtk",
        name: "rtk",
        bin: Bin::Rtk,
    },
    Spec {
        id: "figcraft",
        name: "FigCraft",
        bin: Bin::Figcraft,
    },
    Spec {
        id: "codebase-memory-mcp",
        name: "codebase-memory-mcp",
        bin: Bin::CodebaseMemory,
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedBinaries {
    checked_at_ms: u64,
    binaries: Vec<ExternalBinaryDto>,
}

impl CachedBinaries {
    fn status(&self) -> BinariesStatusDto {
        BinariesStatusDto {
            checked_at_ms: self.checked_at_ms,
            binaries: self.binaries.clone(),
        }
    }
}

/// Probe versions from disk and upstream releases. The cache is one lock, so concurrent callers
/// share a single probe.
pub struct BinariesChecker {
    data_root: PathBuf,
    cache_file: PathBuf,
    client: reqwest::Client,
    gh: PathBuf,
    endpoints: Endpoints,
    cache: Mutex<Option<CachedBinaries>>,
    ttl: Duration,
    refresh_after: Duration,
    probe_timeout: Duration,
}

impl BinariesChecker {
    pub fn new(data_root: &Path) -> Self {
        let cache_file = data_root.join(CACHE_FILE);
        let cache = std::fs::read_to_string(&cache_file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let client = update::client_with_timeout(HTTP_TIMEOUT_SECS).unwrap_or_else(|error| {
            tracing::warn!(%error, "binaries HTTP client build failed; using defaults");
            reqwest::Client::new()
        });
        Self {
            data_root: data_root.to_path_buf(),
            cache_file,
            client,
            gh: std::env::var_os("EMBERLOOP_GH").map_or_else(|| PathBuf::from("gh"), PathBuf::from),
            endpoints: Endpoints::default(),
            cache: Mutex::new(cache),
            ttl: CACHE_TTL,
            refresh_after: REFRESH_AFTER,
            probe_timeout: PROBE_TIMEOUT,
        }
    }

    pub async fn status(&self, refresh: bool) -> BinariesStatusDto {
        let mut cache = self.cache.lock().await;
        let now = now_ms();
        if let Some(cached) = cache.as_ref() {
            if !self.stale(cached.checked_at_ms, now, refresh) {
                return cached.status();
            }
        }
        let binaries = self.refresh().await;
        let cached = CachedBinaries {
            checked_at_ms: now_ms(),
            binaries,
        };
        self.persist(&cached);
        let status = cached.status();
        *cache = Some(cached);
        status
    }

    fn persist(&self, cached: &CachedBinaries) {
        if let Err(error) = write_cache(&self.cache_file, cached) {
            tracing::warn!(%error, path = %self.cache_file.display(), "binaries cache not written");
        }
    }

    fn stale(&self, checked_at_ms: u64, now_ms: u64, refresh: bool) -> bool {
        let age = now_ms.saturating_sub(checked_at_ms);
        age >= self.ttl.as_millis() as u64
            || (refresh && age > self.refresh_after.as_millis() as u64)
    }

    async fn refresh(&self) -> Vec<ExternalBinaryDto> {
        let paths = Paths::resolve(&self.data_root);
        let rows = SPECS.iter().map(|spec| self.row(spec, &paths));
        join_all(rows).await
    }

    async fn row(&self, spec: &Spec, paths: &Paths) -> ExternalBinaryDto {
        let (path, server) = paths.for_bin(spec.bin);
        let installed = self.installed(spec.bin, &path, server.as_deref()).await;
        let latest = self.latest(spec.bin).await;
        let latest_version = latest.as_deref().unwrap_or_default().to_string();
        let (state, installed_version, error) = classify_row(installed, latest);
        ExternalBinaryDto {
            id: spec.id.to_string(),
            name: spec.name.to_string(),
            path: path.display().to_string(),
            installed_version,
            latest_version,
            state,
            error,
        }
    }

    async fn installed(
        &self,
        bin: Bin,
        path: &Path,
        server: Option<&Path>,
    ) -> Result<Installed, String> {
        if bin != Bin::Figcraft {
            return Ok(match self.version(bin, path).await? {
                Some(version) => Installed::Version(version),
                None => Installed::Missing,
            });
        }
        let cli = self.version(bin, path).await?;
        let server = match server {
            Some(server) => self.version(bin, server).await?,
            None => None,
        };
        Ok(figcraft_versions(cli, server))
    }

    async fn version(&self, bin: Bin, path: &Path) -> Result<Option<String>, String> {
        if !path.is_file() {
            return Ok(None);
        }
        let output = tokio::time::timeout(
            self.probe_timeout,
            Command::new(path)
                .arg("--version")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| format!("{} --version timed out", path.display()))?
        .map_err(|error| format!("{} --version: {error}", path.display()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() && stdout.trim().is_empty() {
            let detail = stderr.trim();
            return Err(if detail.is_empty() {
                format!("{} --version exited with {}", path.display(), output.status)
            } else {
                format!(
                    "{} --version exited with {}: {detail}",
                    path.display(),
                    output.status
                )
            });
        }
        let text = if stdout.trim().is_empty() {
            stderr.as_ref()
        } else {
            stdout.as_ref()
        };
        parse_version(bin, text)
            .map(Some)
            .ok_or_else(|| format!("{} --version printed no version", path.display()))
    }

    async fn latest(&self, bin: Bin) -> Result<String, String> {
        match bin {
            Bin::Agy => {
                let value = self.value(&self.endpoints.agy_registry).await?;
                value
                    .get("agents")
                    .and_then(Value::as_array)
                    .and_then(|agents| {
                        agents.iter().find(|agent| {
                            agent.get("id").and_then(Value::as_str) == Some("antigravity-acp")
                        })
                    })
                    .and_then(|agent| agent.get("version"))
                    .and_then(Value::as_str)
                    .map(clean_version)
                    .ok_or_else(missing_version)
            }
            Bin::Claude => {
                let value = self.value(&self.endpoints.claude_npm).await?;
                value
                    .get("version")
                    .and_then(Value::as_str)
                    .map(clean_version)
                    .ok_or_else(missing_version)
            }
            Bin::Pi => self.github_latest(&self.endpoints.pi).await,
            Bin::Rtk => self.github_latest(&self.endpoints.rtk).await,
            Bin::Figcraft => self.github_latest(&self.endpoints.figcraft).await,
            Bin::CodebaseMemory => {
                self.github_latest(&self.endpoints.codebase_memory_mcp)
                    .await
            }
        }
    }

    async fn value(&self, url: &str) -> Result<Value, String> {
        self.get_json(url).await.map_err(|error| error.detail)
    }

    async fn github_latest(&self, repo: &str) -> Result<String, String> {
        let base = self.endpoints.github_api.trim_end_matches('/');
        let url = format!("{base}/repos/{repo}/releases/latest");
        match self.get_json(&url).await {
            Ok(value) => tag_version(&value),
            Err(error) if github_fallback(error.status) => match self.gh_latest(repo).await {
                Ok(version) => Ok(version),
                Err(gh) => Err(format!("{}; gh: {gh}", error.detail)),
            },
            Err(error) => Err(error.detail),
        }
    }

    async fn get_json(&self, url: &str) -> Result<Value, FetchError> {
        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .send()
            .await
            .map_err(|error| FetchError {
                status: None,
                detail: format!("{url}: {error}"),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(FetchError {
                status: Some(status.as_u16()),
                detail: format!("HTTP {status} from {url}"),
            });
        }
        response.json().await.map_err(|error| FetchError {
            status: None,
            detail: format!("{url}: {error}"),
        })
    }

    async fn gh_latest(&self, repo: &str) -> Result<String, String> {
        let mut command = Command::new(&self.gh);
        command
            .args(["api", &format!("repos/{repo}/releases/latest")])
            .kill_on_drop(true);
        let output = tokio::time::timeout(self.probe_timeout, command.output())
            .await
            .map_err(|_| "gh api timed out".to_string())?
            .map_err(|error| format!("gh api: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim();
            return Err(if detail.is_empty() {
                format!("gh api exited with {}", output.status)
            } else {
                format!("gh api: {detail}")
            });
        }
        let value: Value =
            serde_json::from_slice(&output.stdout).map_err(|error| format!("gh api: {error}"))?;
        tag_version(&value)
    }
}

#[async_trait::async_trait]
impl BinaryChecker for BinariesChecker {
    async fn binaries(&self, refresh: bool) -> BinariesStatusDto {
        self.status(refresh).await
    }
}

/// The `update --check` table: one line per binary with both versions and its state.
pub fn table(status: &BinariesStatusDto) -> String {
    let mut out = String::from("external binaries:\n");
    for binary in &status.binaries {
        let installed = dash(&binary.installed_version);
        let latest = dash(&binary.latest_version);
        let state = match binary.state {
            ExternalBinaryState::UpToDate => "up to date",
            ExternalBinaryState::Outdated => "update available",
            ExternalBinaryState::Missing => "missing",
            ExternalBinaryState::Error => "error",
        };
        let detail = binary
            .error
            .as_deref()
            .map(|error| format!(" ({error})"))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {:<20} {installed:<14} {latest:<14} {state}{detail}\n",
            binary.id
        ));
    }
    out
}

fn dash(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

fn write_cache(path: &Path, cached: &CachedBinaries) -> Result<(), String> {
    let text =
        serde_json::to_string_pretty(cached).map_err(|error| format!("not serialised: {error}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

fn clean_version(value: &str) -> String {
    value.trim().trim_start_matches('v').to_string()
}

fn parse_version(bin: Bin, text: &str) -> Option<String> {
    let token = match bin {
        Bin::Agy => text
            .split_once("Build label:")?
            .1
            .split_whitespace()
            .next()?,
        Bin::Claude => text.split_whitespace().next()?,
        Bin::Pi | Bin::Rtk | Bin::Figcraft | Bin::CodebaseMemory => {
            text.split_whitespace().last()?
        }
    };
    Some(clean_version(token))
}

fn classify(installed: &str, latest: &str) -> ExternalBinaryState {
    match update::compare(latest, installed) {
        std::cmp::Ordering::Greater => ExternalBinaryState::Outdated,
        _ => ExternalBinaryState::UpToDate,
    }
}

enum Installed {
    Missing,
    Version(String),
    Mismatch(String),
}

fn figcraft_versions(cli: Option<String>, server: Option<String>) -> Installed {
    match (cli, server) {
        (Some(cli), Some(server)) if cli != server => {
            Installed::Mismatch(format!("cli {cli} / server {server}"))
        }
        (Some(version), Some(_)) => Installed::Version(version),
        _ => Installed::Missing,
    }
}

fn classify_row(
    installed: Result<Installed, String>,
    latest: Result<String, String>,
) -> (ExternalBinaryState, String, Option<String>) {
    match installed {
        Err(error) => (ExternalBinaryState::Error, String::new(), Some(error)),
        Ok(Installed::Missing) => (ExternalBinaryState::Missing, String::new(), latest.err()),
        Ok(Installed::Mismatch(versions)) => (
            ExternalBinaryState::Error,
            versions,
            Some("figcraft-cli and figcraft-server versions differ".to_string()),
        ),
        Ok(Installed::Version(version)) => match latest {
            Ok(latest) => (classify(&version, &latest), version, None),
            Err(error) => (ExternalBinaryState::Error, version, Some(error)),
        },
    }
}

fn tag_version(value: &Value) -> Result<String, String> {
    value
        .get("tag_name")
        .and_then(Value::as_str)
        .map(clean_version)
        .ok_or_else(missing_version)
}

fn missing_version() -> String {
    "missing version field".to_string()
}

fn github_fallback(status: Option<u16>) -> bool {
    matches!(status, Some(403 | 429 | 500..=599))
}

#[derive(Debug)]
struct FetchError {
    status: Option<u16>,
    detail: String,
}

struct Paths {
    agy: PathBuf,
    claude: PathBuf,
    pi: PathBuf,
    rtk: Option<PathBuf>,
    figcraft_cli: Option<PathBuf>,
    figcraft_server: Option<PathBuf>,
    codebase_memory_mcp: Option<PathBuf>,
}

impl Paths {
    fn resolve(data_root: &Path) -> Self {
        let bins = crate::resolve_bin_paths(data_root);
        Self {
            agy: executable(&bins.agy).unwrap_or(bins.agy),
            claude: executable(&bins.claude).unwrap_or(bins.claude),
            pi: executable(&bins.pi).unwrap_or(bins.pi),
            rtk: which("rtk"),
            figcraft_cli: which("figcraft-cli"),
            figcraft_server: which("figcraft-server"),
            codebase_memory_mcp: tools::graph::native_binary(&tools::graph::default_bin_dirs()),
        }
    }

    fn for_bin(&self, bin: Bin) -> (PathBuf, Option<PathBuf>) {
        match bin {
            Bin::Agy => (self.agy.clone(), None),
            Bin::Claude => (self.claude.clone(), None),
            Bin::Pi => (self.pi.clone(), None),
            Bin::Rtk => (
                self.rtk.clone().unwrap_or_else(|| PathBuf::from("rtk")),
                None,
            ),
            Bin::Figcraft => (
                self.figcraft_cli
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("figcraft-cli")),
                self.figcraft_server.clone(),
            ),
            Bin::CodebaseMemory => (
                self.codebase_memory_mcp
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("codebase-memory-mcp")),
                None,
            ),
        }
    }
}

fn executable(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let name = path.file_name()?.to_str()?;
    (path.components().count() == 1)
        .then(|| which(name))
        .flatten()
}

fn which(name: &str) -> Option<PathBuf> {
    tools::graph::default_bin_dirs()
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_per_binary() {
        assert_eq!(
            parse_version(Bin::Agy, "Build label: 1.5.0\nother").as_deref(),
            Some("1.5.0")
        );
        assert_eq!(
            parse_version(Bin::Claude, "2.1.327 (Claude Code)").as_deref(),
            Some("2.1.327")
        );
        assert_eq!(
            parse_version(Bin::Pi, "pi v0.12.16").as_deref(),
            Some("0.12.16")
        );
        assert_eq!(
            parse_version(Bin::Rtk, "rtk 1.2.0").as_deref(),
            Some("1.2.0")
        );
        assert_eq!(parse_version(Bin::Agy, "no label here"), None);
    }

    #[test]
    fn version_state_follows_the_release_order() {
        assert_eq!(classify("1.5.0", "1.5.0"), ExternalBinaryState::UpToDate);
        assert_eq!(classify("1.4.5", "1.5.0"), ExternalBinaryState::Outdated);
        assert_eq!(
            classify("v0.12.16", "0.12.16"),
            ExternalBinaryState::UpToDate
        );
    }

    #[test]
    fn a_figcraft_cli_server_mismatch_is_an_error() {
        let installed = figcraft_versions(Some("0.18.1".into()), Some("0.18.0".into()));
        assert!(matches!(installed, Installed::Mismatch(_)));
        match classify_row(Ok(installed), Ok("0.18.1".into())) {
            (ExternalBinaryState::Error, versions, Some(_)) => {
                assert_eq!(versions, "cli 0.18.1 / server 0.18.0");
            }
            _ => panic!("a mismatch is an error row"),
        }
        assert!(matches!(
            figcraft_versions(Some("0.18.1".into()), None),
            Installed::Missing
        ));
        assert!(matches!(
            figcraft_versions(Some("0.18.1".into()), Some("0.18.1".into())),
            Installed::Version(version) if version == "0.18.1"
        ));
    }

    #[test]
    fn refresh_probes_again_only_after_its_guards() {
        let dir = tempfile::tempdir().unwrap();
        let mut checker = BinariesChecker::new(dir.path());
        checker.ttl = Duration::from_secs(3600);
        checker.refresh_after = Duration::from_secs(10);
        assert!(!checker.stale(1_000, 1_000 + 9_999, true));
        assert!(!checker.stale(1_000, 1_000 + 10_000, true));
        assert!(checker.stale(1_000, 1_000 + 10_001, true));
        assert!(!checker.stale(1_000, 1_000 + 10_001, false));
        assert!(checker.stale(1_000, 1_000 + 3_600_000, false));
    }

    #[test]
    fn a_persisted_cache_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let cached = CachedBinaries {
            checked_at_ms: 7,
            binaries: Vec::new(),
        };
        std::fs::write(
            dir.path().join(CACHE_FILE),
            serde_json::to_string(&cached).unwrap(),
        )
        .unwrap();
        let checker = BinariesChecker::new(dir.path());
        let loaded = checker.cache.try_lock().unwrap();
        assert_eq!(loaded.as_ref().map(|cached| cached.checked_at_ms), Some(7));
    }

    #[test]
    fn a_missing_or_unreachable_latest_keeps_the_row_state() {
        assert_eq!(
            classify_row(Ok(Installed::Missing), Err("offline".into())),
            (
                ExternalBinaryState::Missing,
                String::new(),
                Some("offline".into())
            )
        );
        assert_eq!(
            classify_row(
                Ok(Installed::Version("1.0.0".into())),
                Err("offline".into())
            ),
            (
                ExternalBinaryState::Error,
                "1.0.0".into(),
                Some("offline".into())
            )
        );
    }

    #[test]
    fn the_check_table_lists_every_row_with_its_state() {
        let row = |id: &str, installed: &str, latest: &str, state, error: Option<&str>| {
            ExternalBinaryDto {
                id: id.into(),
                name: id.into(),
                path: String::new(),
                installed_version: installed.into(),
                latest_version: latest.into(),
                state,
                error: error.map(str::to_string),
            }
        };
        let status = BinariesStatusDto {
            checked_at_ms: 0,
            binaries: vec![
                row(
                    "pi",
                    "0.12.16",
                    "0.12.16",
                    ExternalBinaryState::UpToDate,
                    None,
                ),
                row("figcraft", "", "0.18.1", ExternalBinaryState::Missing, None),
                row(
                    "claude",
                    "2.1.319",
                    "",
                    ExternalBinaryState::Error,
                    Some("offline"),
                ),
            ],
        };
        let table = table(&status);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "external binaries:");
        assert!(
            lines[1].contains("pi") && lines[1].contains("up to date"),
            "{table}"
        );
        assert!(
            lines[2].contains("figcraft")
                && lines[2].contains("0.18.1")
                && lines[2].contains("missing"),
            "{table}"
        );
        assert!(
            lines[3].contains("claude")
                && lines[3].contains("2.1.319")
                && lines[3].contains("error (offline)"),
            "{table}"
        );
    }

    #[test]
    fn a_probe_result_is_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nested");
        let checker = BinariesChecker::new(&root);
        checker.persist(&CachedBinaries {
            checked_at_ms: 9,
            binaries: Vec::new(),
        });
        let raw = std::fs::read_to_string(root.join(CACHE_FILE)).unwrap();
        let loaded: CachedBinaries = serde_json::from_str(&raw).unwrap();
        assert_eq!(loaded.checked_at_ms, 9);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_version_probe_runs_the_binary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("figcraft-cli");
        std::fs::write(&bin, "#!/bin/sh\necho 0.18.1\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let checker = BinariesChecker::new(dir.path());
        assert_eq!(
            checker
                .version(Bin::Figcraft, &bin)
                .await
                .unwrap()
                .as_deref(),
            Some("0.18.1")
        );
        let broken = dir.path().join("broken");
        std::fs::write(&broken, "#!/bin/sh\necho boom >&2\nexit 3\n").unwrap();
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = checker.version(Bin::Pi, &broken).await.unwrap_err();
        assert!(
            error.contains("exited with") && error.contains("3") && error.contains("boom"),
            "{error}"
        );
        assert_eq!(
            checker
                .version(Bin::Pi, &dir.path().join("absent"))
                .await
                .unwrap(),
            None
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_rate_limited_github_api_falls_back_to_gh() {
        use std::os::unix::fs::PermissionsExt;

        use axum::routing::get;

        let app = axum::Router::new().route(
            "/repos/{owner}/{repo}/releases/latest",
            get(|| async { axum::http::StatusCode::FORBIDDEN }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(&gh, "#!/bin/sh\nprintf '%s' '{\"tag_name\":\"v9.9.9\"}'\n").unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut checker = BinariesChecker::new(dir.path());
        checker.endpoints.github_api = format!("http://{addr}");
        checker.endpoints.pi = "o/r".into();
        checker.gh = gh;
        assert_eq!(checker.latest(Bin::Pi).await.unwrap(), "9.9.9");
        server.abort();
    }
}
