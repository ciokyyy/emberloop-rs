#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! Daemon composition root: stores, prompts, API, proxy, tools and the agy
//! harness. The binary in `main.rs` is a thin CLI over this module.

use std::io::Read;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::AppState;
use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use clap::{Parser, Subcommand};
use emberloop_core::context::{Context, ContextError, Plugin};
use emberloop_core::events::EventBus;
use harness::HarnessWiring;
use harness_agy::AgyHarness;
use harness_pi::PiHarness;
use prompts::{PromptStore, PromptWatcher};
use proxy::{ProxyConfig, ProxyState};
use rust_embed::RustEmbed;
use store::{DataRoot, ProjectsStore, SessionsStore};
use tools::{McpServer, ToolConfig, Toolbox};

pub mod login;
pub mod runner;

pub fn is_pi_model(model: Option<&str>) -> bool {
    model.is_some_and(|model| model.starts_with("opencode-go/"))
}

struct MultiHarness {
    agy: Arc<dyn harness::Harness>,
    pi: Arc<dyn harness::Harness>,
}

#[async_trait::async_trait]
impl harness::Harness for MultiHarness {
    async fn spawn(
        &self,
        session_id: &str,
        wiring: HarnessWiring,
    ) -> Result<Box<dyn harness::HarnessSession>, harness::HarnessError> {
        if is_pi_model(wiring.model.as_deref()) {
            self.pi.spawn(session_id, wiring).await
        } else {
            self.agy.spawn(session_id, wiring).await
        }
    }
}

/// The harness plugin constructs the agy and pi drivers and publishes the
/// composite harness service on the Context tree.
struct HarnessPlugin {
    agy_bin: PathBuf,
    agy_home: PathBuf,
    pi: Arc<PiHarness>,
}

impl Plugin for HarnessPlugin {
    fn name(&self) -> &str {
        "harness"
    }

    fn apply(&self, ctx: &Context) -> Result<(), ContextError> {
        let agy = Arc::new(AgyHarness::new(self.agy_bin.clone(), self.agy_home.clone()));
        let pi: Arc<dyn harness::Harness> = self.pi.clone();
        ctx.provide::<Arc<dyn harness::Harness>>(Arc::new(MultiHarness { agy, pi }));
        Ok(())
    }
}

struct PiSource {
    harness: Arc<PiHarness>,
}

#[async_trait::async_trait]
impl api::models::Source for PiSource {
    fn name(&self) -> &'static str {
        "pi"
    }

    async fn fetch(&self) -> Result<Vec<api::models::ModelEntry>, api::models::CatalogueError> {
        let models = self
            .harness
            .models()
            .await
            .map_err(|error| api::models::CatalogueError::new(error.to_string()))?;
        Ok(models
            .into_iter()
            .map(|(id, label)| api::models::ModelEntry {
                id,
                provider: "opencode-go".to_string(),
                label: label
                    .strip_prefix("opencode-go: ")
                    .unwrap_or(&label)
                    .to_string(),
                kind: "chat".to_string(),
                efforts: vec!["low".into(), "medium".into(), "high".into()],
                image_input: true,
                context_window: 1_000_000,
                source: "pi".to_string(),
                stale: false,
                updated_at: 0,
            })
            .collect())
    }
}

#[derive(RustEmbed)]
#[folder = "../../web/build"]
struct Assets;

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub host: String,
    pub data_root: PathBuf,
    pub config_root: PathBuf,
    pub upstream: Option<String>,
    pub allow_remote: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 3000,
            host: "127.0.0.1".to_string(),
            data_root: default_data_root(),
            config_root: default_config_root(),
            upstream: None,
            allow_remote: false,
        }
    }
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !is_loopback(&self.host) && !self.allow_remote {
            anyhow::bail!(
                "host {} is not loopback; pass --allow-remote to bind it",
                self.host
            );
        }
        Ok(())
    }

    pub fn from_cli(cli: &Cli) -> Self {
        let mut config = Self::default();
        if let Some(port) = cli.port {
            config.port = port;
        }
        if let Some(host) = &cli.host {
            config.host = host.clone();
        }
        if let Some(data_root) = &cli.data_root {
            config.data_root = data_root.clone();
        }
        if let Some(config_root) = &cli.config_root {
            config.config_root = config_root.clone();
        }
        config.upstream = cli.upstream.clone();
        config.allow_remote = cli.allow_remote;
        config
    }
}

pub fn default_data_root() -> PathBuf {
    home().join(".local/share/emberloop")
}

pub fn default_config_root() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("emberloop")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn resolve_bin(env_var: &str, data_root: &std::path::Path, rel: &str, fallback: &str) -> PathBuf {
    if let Some(bin) = std::env::var_os(env_var) {
        return PathBuf::from(bin);
    }
    for root in [data_root.to_path_buf(), default_data_root()] {
        let candidate = root.join(rel);
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(fallback)
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

#[derive(Debug, Parser)]
#[command(name = "emberloop-daemon", version, about = "emberloop-rs daemon")]
pub struct Cli {
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long)]
    pub host: Option<String>,
    #[arg(long)]
    pub data_root: Option<PathBuf>,
    #[arg(long)]
    pub config_root: Option<PathBuf>,
    #[arg(long)]
    pub upstream: Option<String>,
    #[arg(long)]
    pub allow_remote: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Serve one Session's tools over MCP stdio.
    Mcp {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        spill: PathBuf,
    },
}

pub struct Daemon {
    config: Config,
    pub data_root: DataRoot,
    pub projects: ProjectsStore,
    pub sessions: SessionsStore,
    pub prompts: Arc<PromptStore>,
    pub app_state: AppState,
    pub proxy_state: ProxyState,
    harness: Arc<dyn harness::Harness>,
    _watcher: PromptWatcher,
}

impl Daemon {
    pub async fn open(config: Config) -> anyhow::Result<Self> {
        config.validate()?;
        let data_root = DataRoot::new(config.data_root.clone());
        data_root.ensure_layout().await?;
        std::fs::create_dir_all(config.config_root.join("prompts"))?;

        let projects = ProjectsStore::load(data_root.projects_file()).await?;
        let sessions = SessionsStore::load(data_root.clone()).await?;
        let prompts = Arc::new(PromptStore::new(config.config_root.join("prompts")));
        let watcher = prompts::watch(Arc::clone(&prompts), Duration::from_millis(500), |error| {
            tracing::warn!(%error, "prompt reload failed; keeping last good text");
        });

        let events = Arc::new(EventBus::default());
        let agy_bin = resolve_bin(
            "EMBERLOOP_AGY_BIN",
            &config.data_root,
            "agy-acp/1.4.5/agy_acp_server.par",
            "agy",
        );
        let pi_bin = resolve_bin(
            "EMBERLOOP_PI_BIN",
            &config.data_root,
            "pi/v0.12.11/pi",
            "pi",
        );
        let keys_file = config.config_root.join("keys.json");
        let pi_harness = Arc::new(PiHarness::new(
            pi_bin,
            data_root.root().join("pi-homes"),
            keys_file.clone(),
        ));
        let models = Arc::new(api::models::ModelCatalogue::new(
            data_root.model_catalogue_file(),
            vec![
                Arc::new(api::models::AgySource),
                Arc::new(PiSource {
                    harness: Arc::clone(&pi_harness),
                }),
            ],
        ));
        let mut app_state = AppState::with_prompts(
            data_root.clone(),
            projects.clone(),
            sessions.clone(),
            Arc::clone(&events),
            Arc::clone(&prompts),
        )
        .with_models(models);
        app_state.models.spawn_refresher();
        let mut proxy_config = ProxyConfig::new(random_secret());
        if let Some(upstream) = &config.upstream {
            proxy_config = proxy_config.with_upstream(upstream.clone());
        }
        let proxy_state = ProxyState::new(
            proxy_config,
            sessions.clone(),
            projects.clone(),
            data_root.clone(),
            Arc::clone(&prompts),
            Arc::clone(&events),
            app_state.steers.clone(),
        );

        let accounts_dir = config.config_root.join("accounts/agy");
        let ctx = Context::new();
        ctx.plugin(HarnessPlugin {
            agy_bin: agy_bin.clone(),
            agy_home: accounts_dir.join("default"),
            pi: pi_harness,
        });
        let harness = Arc::unwrap_or_clone(
            ctx.require::<Arc<dyn harness::Harness>>()
                .map_err(|error| anyhow::anyhow!(error))?,
        );
        let login = Arc::new(login::AgyLogin::new(accounts_dir, agy_bin).with_keys(keys_file));
        let proxy_base = format!("http://{}:{}", config.host, config.port);
        let runner = Arc::new(runner::SessionRunner::new(
            harness.clone(),
            sessions.clone(),
            projects.clone(),
            data_root.clone(),
            Arc::clone(&events),
            app_state.steers.clone(),
            proxy_state.clone(),
            proxy_base,
            Arc::clone(&app_state.event_logs),
            Some(config.config_root.join("accounts/agy/default")),
        ));
        app_state = app_state.with_driver(runner).with_provider_login(login);
        api::persist::spawn_persister(app_state.clone());

        spawn_archival_sweep(data_root.clone(), sessions.clone());

        Ok(Self {
            config,
            data_root,
            projects,
            sessions,
            prompts,
            app_state,
            proxy_state,
            harness,
            _watcher: watcher,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn prompts(&self) -> Arc<PromptStore> {
        Arc::clone(&self.prompts)
    }

    pub fn reload_prompts(&self) {
        for error in self.prompts.reload() {
            tracing::warn!(%error, "prompt reload failed; keeping last good text");
        }
    }

    pub fn token_for(&self, session_id: &str) -> String {
        self.proxy_state.token_for(session_id)
    }

    pub fn harness(&self) -> &dyn harness::Harness {
        self.harness.as_ref()
    }

    pub fn router(&self) -> Router {
        api::build_router(self.app_state.clone())
            .merge(proxy::router(self.proxy_state.clone()))
            .fallback(static_asset)
    }

    pub async fn toolbox_for(&self, session_id: &str) -> anyhow::Result<Option<Toolbox>> {
        let Some(meta) = self.sessions.get(session_id).await else {
            return Ok(None);
        };
        let Some(project) = self.projects.get(&meta.project_id).await else {
            return Ok(None);
        };
        let toolbox = Toolbox::new(ToolConfig::new(
            project.root,
            self.data_root.session_spill_dir(session_id),
        ))?;
        toolbox.flush_notices().await?;
        Ok(Some(toolbox))
    }

    pub async fn mcp_server_for(&self, session_id: &str) -> anyhow::Result<Option<McpServer>> {
        let Some(toolbox) = self.toolbox_for(session_id).await? else {
            return Ok(None);
        };
        Ok(Some(McpServer::new(Arc::new(toolbox))))
    }

    pub async fn harness_wiring(&self, session_id: &str) -> anyhow::Result<Option<HarnessWiring>> {
        let Some(meta) = self.sessions.get(session_id).await else {
            return Ok(None);
        };
        let Some(project) = self.projects.get(&meta.project_id).await else {
            return Ok(None);
        };
        let token = self.token_for(session_id);
        let proxy_url = format!("http://{}:{}/p/{token}", self.config.host, self.config.port);
        let spill = self.data_root.session_spill_dir(session_id);
        let mcp_command = daemon_exe().map(|exe| {
            vec![
                exe.display().to_string(),
                "mcp".to_string(),
                "--root".to_string(),
                project.root.display().to_string(),
                "--spill".to_string(),
                spill.display().to_string(),
            ]
        });
        Ok(Some(HarnessWiring {
            proxy_url,
            mcp_url: None,
            mcp_command,
            env: Vec::new(),
            model: Some(meta.model.clone()),
            resume_token: None,
            workdir: None,
        }))
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        let app = self.router();
        let listener = listen(&self.config).await?;
        warn_if_remote(&self.config);
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        Ok(())
    }
}

/// The path of this daemon's own binary. Linux appends " (deleted)" to it once
/// a rebuild replaces the file under a running daemon; the new file at the
/// same path is what agy must launch for the tool server.
pub fn daemon_exe() -> Option<PathBuf> {
    std::env::current_exe().ok().map(strip_deleted)
}

fn strip_deleted(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_suffix(" (deleted)") {
        Some(live) => PathBuf::from(live),
        None => path,
    }
}

fn spawn_archival_sweep(data_root: DataRoot, sessions: SessionsStore) {
    store::ArchivalSweeper::new(data_root, sessions, store::ArchivalConfig::default())
        .spawn_scheduler(log_sweep);
}

fn log_sweep(report: Result<store::SweepReport, store::ArchivalError>) {
    match report {
        Ok(report) => sweep_done(&report),
        Err(error) => sweep_failed(&error),
    }
}

fn sweep_done(report: &store::SweepReport) {
    tracing::info!(?report, "archival sweep finished");
}

fn sweep_failed(error: &store::ArchivalError) {
    tracing::warn!(%error, "archival sweep failed");
}

async fn listen(config: &Config) -> anyhow::Result<tokio::net::TcpListener> {
    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "emberloop daemon listening");
    Ok(listener)
}

fn warn_if_remote(config: &Config) {
    if !is_loopback(&config.host) {
        tracing::warn!(host = %config.host, "daemon bound to a non-loopback address");
    }
}

pub async fn run_mcp(root: PathBuf, spill: PathBuf) -> anyhow::Result<()> {
    let toolbox = Toolbox::new(ToolConfig::new(root, spill))?;
    toolbox.flush_notices().await?;
    Arc::new(McpServer::new(Arc::new(toolbox)))
        .serve_stdio()
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "failed to listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received");
}

fn random_secret() -> Vec<u8> {
    let mut bytes = [0u8; 32];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        if file.read_exact(&mut bytes).is_ok() {
            return bytes.to_vec();
        }
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seed = format!(
        "{now}:{}:{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    seed.bytes().cycle().take(bytes.len()).collect()
}

async fn static_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") || path.starts_with("p/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let requested = if path.is_empty() { "index.html" } else { path };
    let served = if Assets::get(requested).is_some() {
        requested
    } else {
        "index.html"
    };
    let Some(asset) = Assets::get(served) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(served))
        .body(Body::from(asset.data.into_owned()))
    {
        Ok(response) => response,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod exe_tests {
    use super::*;

    #[test]
    fn a_replaced_binary_keeps_its_launchable_path() {
        let replaced = PathBuf::from("/work/target/debug/emberloop-daemon (deleted)");
        assert_eq!(
            strip_deleted(replaced),
            PathBuf::from("/work/target/debug/emberloop-daemon")
        );
        let live = PathBuf::from("/work/target/debug/emberloop-daemon");
        assert_eq!(strip_deleted(live.clone()), live);
    }
}
