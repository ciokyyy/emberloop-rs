#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! Daemon composition root: stores, prompts, API, proxy, tools and the agy
//! harness. The binary in `main.rs` is a thin CLI over this module.

use std::collections::HashMap;
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
use emberloop_core::events::{EventBus, EventKind, SessionEvent};
use harness::HarnessWiring;
use harness_agy::AgyHarness;
use harness_claude::ClaudeHarness;
use harness_pi::PiHarness;
use prompts::{PromptStore, PromptWatcher};
use proxy::{ProxyConfig, ProxyState};
use rust_embed::RustEmbed;
use store::{DataRoot, ProjectsStore, SessionsStore};
use tools::{McpServer, ToolConfig, Toolbox};

pub mod claude_login;
pub mod codegraph;
pub mod codex;
pub mod figcraft;
pub mod idle;
pub mod login;
mod review;
pub mod runner;
mod titles;
pub mod todo_reminder;
pub mod tooldisplay;
pub mod update;
mod usage;

/// Which harness serves a model id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HarnessKind {
    Agy,
    Pi,
    Claude,
}

pub fn harness_kind(model: Option<&str>) -> HarnessKind {
    if model.is_some_and(|model| model.starts_with("claude/")) {
        HarnessKind::Claude
    } else if is_pi_model(model) {
        HarnessKind::Pi
    } else {
        HarnessKind::Agy
    }
}

pub fn is_pi_model(model: Option<&str>) -> bool {
    model
        .and_then(|model| model.split_once('/'))
        .is_some_and(|(provider, _)| {
            matches!(provider, "opencode-go" | "openai-codex")
                || emberloop_core::providers::key_provider(provider).is_some()
        })
}

struct MultiHarness {
    agy: Arc<dyn harness::Harness>,
    pi: Arc<dyn harness::Harness>,
    claude: Arc<dyn harness::Harness>,
}

#[async_trait::async_trait]
impl harness::Harness for MultiHarness {
    async fn spawn(
        &self,
        session_id: &str,
        wiring: HarnessWiring,
    ) -> Result<Box<dyn harness::HarnessSession>, harness::HarnessError> {
        match harness_kind(wiring.model.as_deref()) {
            HarnessKind::Claude => self.claude.spawn(session_id, wiring).await,
            HarnessKind::Pi => self.pi.spawn(session_id, wiring).await,
            HarnessKind::Agy => self.agy.spawn(session_id, wiring).await,
        }
    }
}

/// The harness plugin constructs the agy and pi drivers and publishes the
/// composite harness service on the Context tree.
struct HarnessPlugin {
    agy: Arc<AgyHarness>,
    pi: Arc<PiHarness>,
    claude: Arc<ClaudeHarness>,
}

impl Plugin for HarnessPlugin {
    fn name(&self) -> &str {
        "harness"
    }

    fn apply(&self, ctx: &Context) -> Result<(), ContextError> {
        let agy = Arc::clone(&self.agy);
        let pi: Arc<dyn harness::Harness> = self.pi.clone();
        let claude: Arc<dyn harness::Harness> = self.claude.clone();
        ctx.provide::<Arc<dyn harness::Harness>>(Arc::new(MultiHarness { agy, pi, claude }));
        Ok(())
    }
}

/// The fixed model aliases the real `claude` CLI accepts; the CLI resolves each
/// alias to the newest model its subscription serves.
struct ClaudeSource;

#[async_trait::async_trait]
impl api::models::Source for ClaudeSource {
    fn name(&self) -> &'static str {
        "claude"
    }

    async fn fetch(&self) -> Result<Vec<api::models::ModelEntry>, api::models::CatalogueError> {
        const ALL: &[&str] = &["low", "medium", "high", "xhigh", "max"];
        const LEGACY: &[&str] = &["low", "medium", "high", "max"];
        // shortcut: snapshot of CLI 2.1.319 initialize.models, use discovery when the CLI catalogue changes.
        Ok([
            ("opus", "Claude Opus 5.5", ALL),
            ("sonnet", "Claude Sonnet 5.5", ALL),
            ("claude-fable-5-2", "Claude Fable 5.2", ALL),
            ("haiku", "Claude Haiku 5.5", ALL),
            ("fable", "Claude Fable 5.1", ALL),
            ("claude-haiku-4-5-20251001", "Claude Haiku 4.5", &[]),
            ("claude-sonnet-5", "Claude Sonnet 5", ALL),
            ("claude-opus-5", "Claude Opus 5", ALL),
            ("claude-fable-5", "Claude Fable 5", ALL),
            ("claude-opus-4-8", "Claude Opus 4.8", ALL),
            ("claude-opus-4-7", "Claude Opus 4.7", ALL),
            ("claude-opus-4-6", "Claude Opus 4.6", LEGACY),
            ("claude-sonnet-4-6", "Claude Sonnet 4.6", LEGACY),
        ]
        .into_iter()
        .map(|(alias, label, efforts)| api::models::ModelEntry {
            id: format!("claude/{alias}"),
            provider: "claude".to_string(),
            label: label.to_string(),
            kind: "chat".to_string(),
            efforts: efforts.iter().map(ToString::to_string).collect(),
            image_input: true,
            context_window: if alias == "claude-haiku-4-5-20251001" {
                200_000
            } else {
                1_000_000
            },
            source: "claude".to_string(),
            stale: false,
            updated_at: 0,
        })
        .collect())
    }
}

struct AgySource {
    harness: Arc<AgyHarness>,
}

#[async_trait::async_trait]
impl api::models::Source for AgySource {
    fn name(&self) -> &'static str {
        "agy"
    }

    async fn fetch(&self) -> Result<Vec<api::models::ModelEntry>, api::models::CatalogueError> {
        let models = self
            .harness
            .models()
            .await
            .map_err(|error| api::models::CatalogueError::new(error.to_string()))?;
        Ok(models
            .into_iter()
            .map(|model| api::models::ModelEntry {
                id: model.id,
                provider: "agy".to_string(),
                label: model.label,
                kind: "chat".to_string(),
                efforts: model.efforts,
                image_input: model.image_input,
                context_window: model.context_window,
                source: "agy".to_string(),
                stale: false,
                updated_at: 0,
            })
            .collect())
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
            .catalogue()
            .await
            .map_err(|error| api::models::CatalogueError::new(error.to_string()))?;
        Ok(models
            .into_iter()
            .map(|model| api::models::ModelEntry {
                id: model.id,
                provider: model.provider,
                label: model.label,
                kind: "chat".to_string(),
                efforts: model.efforts,
                image_input: model.image_input,
                context_window: model.context_window,
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
    /// Global skills root (`~/.agents/skills`); every Session's tools may read it.
    pub global_skills: PathBuf,
    pub upstream: Option<String>,
    pub allow_remote: bool,
    /// Idle time before a Session's harness and tool server are closed.
    pub idle_timeout: std::time::Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 3000,
            host: "127.0.0.1".to_string(),
            data_root: default_data_root(),
            config_root: default_config_root(),
            global_skills: default_global_skills(),
            upstream: None,
            allow_remote: false,
            idle_timeout: idle::DEFAULT_IDLE_TIMEOUT,
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
        if let Some(minutes) = cli.idle_timeout_minutes {
            config.idle_timeout = std::time::Duration::from_secs(minutes * 60);
        }
        config
    }
}

pub fn default_data_root() -> PathBuf {
    home().join(".local/share/emberloop-rs")
}

pub fn default_config_root() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("emberloop-rs")
}

pub fn default_global_skills() -> PathBuf {
    home().join(".agents/skills")
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
#[command(
    name = "emberloop-daemon",
    version = crate::update::VERSION,
    about = "emberloop-rs daemon"
)]
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
    /// Minutes a Session may sit idle before its harness is closed; it resumes on its next message.
    #[arg(long)]
    pub idle_timeout_minutes: Option<u64>,
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
        /// Global skills root, readable beyond the project root; omit for none.
        #[arg(long)]
        global_skills: Option<PathBuf>,
    },
    /// Install the newest release under `releases/` and switch `current` to it.
    Update {
        /// Release manifest; defaults to EMBERLOOP_UPDATE_URL or the GitHub release.
        #[arg(long)]
        manifest: Option<String>,
        /// Session to resume once the new binary starts.
        #[arg(long)]
        session: Option<String>,
        /// Report the version available without installing it.
        #[arg(long)]
        check: bool,
        /// Point `current` at the release before it instead of fetching one.
        #[arg(long)]
        rollback: bool,
        /// Install a binary already on this machine (a local build), one version up; the last
        /// number steps by one unless `--major` is given.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Step the first number instead of the last: 0.0.12 becomes 1.0.0.
        #[arg(long)]
        major: bool,
        /// Print the version the next release would carry, then exit.
        #[arg(long)]
        next_version: bool,
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
    /// The Sessions to prompt once the listener is up after an update.
    resume: Vec<(Arc<runner::SessionRunner>, String, String)>,
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
        let accounts_dir = config.config_root.join("accounts/agy");
        let agy_harness = Arc::new(AgyHarness::new(
            agy_bin.clone(),
            accounts_dir.join("default"),
        ));
        let pi_bin = resolve_bin(
            "EMBERLOOP_PI_BIN",
            &config.data_root,
            "pi/v0.12.11/pi",
            "pi",
        );
        let keys_file = config.config_root.join("keys.json");
        let claude_bin = resolve_bin(
            "EMBERLOOP_CLAUDE_BIN",
            &config.data_root,
            "claude/2.1.319/claude",
            "claude",
        );
        let pi_auth = config.config_root.join("pi/agent/auth.json");
        let pi_harness = Arc::new(
            PiHarness::new(pi_bin, data_root.root().join("pi-homes"), keys_file.clone())
                .with_auth(pi_auth.clone()),
        );
        let claude_homes = data_root.root().join("claude-homes");
        let claude_harness = Arc::new(ClaudeHarness::new(claude_bin.clone(), claude_homes.clone()));
        let models = Arc::new(api::models::ModelCatalogue::new(
            data_root.model_catalogue_file(),
            vec![
                Arc::new(AgySource {
                    harness: Arc::clone(&agy_harness),
                }),
                Arc::new(PiSource {
                    harness: Arc::clone(&pi_harness),
                }),
                Arc::new(ClaudeSource),
            ],
        ));
        let mut app_state = AppState::with_prompts(
            data_root.clone(),
            projects.clone(),
            sessions.clone(),
            Arc::clone(&events),
            Arc::clone(&prompts),
        )
        .with_models(models)
        .with_tool_keys_file(config.config_root.join("tool-keys.json"))
        .with_figcraft(config.config_root.join("mcp.json"))
        .with_global_skills(config.global_skills.clone())
        .with_groq_keys(keys_file.clone(), api::transcribe::GROQ_URL);
        app_state.models.spawn_refresher();

        let updated = announce_update(&app_state, &events, &data_root).await;
        let codebase_memory = codegraph::ensure(&config.config_root);
        let _ = figcraft::ensure(&config.config_root);
        let mut proxy_config = ProxyConfig::new(random_secret())
            .with_codebase_memory(codebase_memory)
            .with_review_tools(Arc::new(review::ReviewExecutor {
                sessions: sessions.clone(),
                projects: projects.clone(),
                data_root: data_root.clone(),
                global_skills: config.global_skills.clone(),
                config_root: config.config_root.clone(),
            }));
        if let Some(upstream) = &config.upstream {
            proxy_config = proxy_config.with_upstream(upstream.clone());
        }
        let proxy_state = ProxyState::new(
            proxy_config,
            sessions.clone(),
            projects.clone(),
            data_root.clone(),
            Arc::clone(&prompts),
            Arc::clone(&app_state.models),
            Arc::clone(&events),
            app_state.steers.clone(),
            Arc::clone(&app_state.settings),
        );

        let ctx = Context::new();
        ctx.plugin(HarnessPlugin {
            agy: agy_harness,
            pi: pi_harness,
            claude: claude_harness,
        });
        let harness = Arc::unwrap_or_clone(
            ctx.require::<Arc<dyn harness::Harness>>()
                .map_err(|error| anyhow::anyhow!(error))?,
        );
        let login = Arc::new(
            login::AgyLogin::new(accounts_dir, agy_bin)
                .with_keys(keys_file.clone())
                .with_pi_auth(pi_auth.clone())
                .with_claude(claude_login::ClaudeLogin::new(claude_bin, claude_homes)),
        );
        let proxy_base = format!("http://{}:{}", config.host, config.port);
        let runner = Arc::new(
            runner::SessionRunner::new(
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
                Arc::clone(&app_state.settings),
            )
            .with_titles(titles::Titles::new(
                sessions.clone(),
                Arc::clone(&app_state.settings),
                Arc::clone(&events),
                keys_file,
                pi_auth,
            ))
            .with_models(Arc::clone(&app_state.models))
            .with_config_root(config.config_root.clone())
            .with_global_skills(config.global_skills.clone())
            .with_prompts(Arc::clone(&prompts))
            .with_idle_timeout(config.idle_timeout),
        );
        runner.watch_updates();
        runner.watch_idle();
        runner.watch_background();
        let resume = updated.map_or_else(Vec::new, |pending| {
            let mut resume = Vec::new();
            if let Some(session) = pending.session.clone() {
                resume.push((
                    Arc::clone(&runner),
                    session,
                    update::resume_prompt(&pending),
                ));
            }
            // A Session the swap cut off mid-turn gets a bare `continue`, no update notice, so
            // its history gains only one short turn after the cached prefix.
            for session in &pending.also {
                resume.push((Arc::clone(&runner), session.clone(), "continue".to_string()));
            }
            resume
        });
        app_state = app_state.with_driver(runner).with_provider_login(login);
        api::persist::spawn_persister(app_state.clone());
        api::subagents::watch_errors(app_state.clone());
        api::push::watch(app_state.clone());

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
            resume,
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
            .nest_service(
                "/figcraft",
                tower_http::services::ServeDir::new("/usr/local/share/figcraft"),
            )
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
        let toolbox = Toolbox::new(ToolConfig {
            env_file: Some(self.config.config_root.join("env")),
            tool_keys_file: Some(self.config.config_root.join("tool-keys.json")),
            mcp_file: Some(self.config.config_root.join("mcp.json")),
            global_skills: Some(self.config.global_skills.clone()),
            prompts_dir: Some(self.config.config_root.join("prompts")),
            session_url: Some(format!(
                "http://{}:{}/api/sessions/{session_id}",
                self.config.host, self.config.port
            )),
            reviewer: meta.reviewer.as_ref().is_some_and(|r| r.is_reviewer()),
            subagent_role: meta.reviewer.as_ref().map(|r| r.role.clone()),
            image_input: self
                .app_state
                .models
                .image_input(&meta.model)
                .await
                .unwrap_or(true),
            ctx_spill_enabled: self.app_state.settings.read().await.ctx_spill_enabled,
            ..ToolConfig::new(project.root, self.data_root.session_spill_dir(session_id))
        })?;
        toolbox.flush_notices().await?;
        Ok(Some(toolbox))
    }

    pub async fn mcp_server_for(&self, session_id: &str) -> anyhow::Result<Option<McpServer>> {
        let Some(toolbox) = self.toolbox_for(session_id).await? else {
            return Ok(None);
        };
        Ok(Some(
            McpServer::new(Arc::new(toolbox)).with_descriptions(tool_descriptions(&self.prompts)),
        ))
    }

    pub async fn harness_wiring(&self, session_id: &str) -> anyhow::Result<Option<HarnessWiring>> {
        let Some(meta) = self.sessions.get(session_id).await else {
            return Ok(None);
        };
        let Some(project) = self.projects.get(&meta.project_id).await else {
            return Ok(None);
        };
        let token = self.token_for(session_id);
        let proxy_url = format!(
            "http://{}:{}/proxy/{token}",
            self.config.host, self.config.port
        );
        let spill = self.data_root.session_spill_dir(session_id);
        let mcp_command = daemon_exe().map(|exe| {
            vec![
                exe.display().to_string(),
                "--config-root".to_string(),
                self.config.config_root.display().to_string(),
                "mcp".to_string(),
                "--root".to_string(),
                project.root.display().to_string(),
                "--spill".to_string(),
                spill.display().to_string(),
                "--global-skills".to_string(),
                self.config.global_skills.display().to_string(),
            ]
        });
        Ok(Some(HarnessWiring {
            proxy_url,
            mcp_url: None,
            mcp_command,
            env: {
                let mut env = vec![
                    (
                        "EMBERLOOP_SESSION_URL".into(),
                        format!(
                            "http://{}:{}/api/sessions/{session_id}",
                            self.config.host, self.config.port
                        ),
                    ),
                    (
                        "EMBERLOOP_IMAGE_INPUT".into(),
                        if self
                            .app_state
                            .models
                            .image_input(&meta.model)
                            .await
                            .unwrap_or(true)
                        {
                            "1"
                        } else {
                            "0"
                        }
                        .into(),
                    ),
                    (
                        "EMBERLOOP_CTX_SPILL".into(),
                        if self.app_state.settings.read().await.ctx_spill_enabled {
                            "1"
                        } else {
                            "0"
                        }
                        .into(),
                    ),
                ];
                Self::push_subagent_env(&mut env, &meta);
                env
            },
            model: Some(meta.model.clone()),
            thought_level: meta.thought_level.clone(),
            resume_token: None,
            workdir: None,
        }))
    }

    pub(crate) fn push_subagent_env(env: &mut Vec<(String, String)>, meta: &store::SessionMeta) {
        let is_reviewer = meta.reviewer.as_ref().is_some_and(|r| r.is_reviewer());
        let role = meta
            .reviewer
            .as_ref()
            .map(|r| r.role.clone())
            .unwrap_or_default();
        env.push((
            "EMBERLOOP_REVIEWER".into(),
            if is_reviewer { "1" } else { "0" }.into(),
        ));
        env.push(("EMBERLOOP_SUBAGENT_ROLE".into(), role));
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        let _figcraft = match figcraft::start(&self.config.config_root).await {
            Ok(child) => Some(child),
            Err(error) => {
                tracing::warn!(%error, "FigCraft unavailable");
                None
            }
        };
        let app = self.router();
        let listener = listen(&self.config).await?;
        warn_if_remote(&self.config);
        let recovery = self.app_state.clone();
        tokio::spawn(async move {
            if let Err(error) = api::subagents::recover(&recovery).await {
                tracing::error!(?error, "could not recover reviewer report delivery");
            }
        });
        for (runner, session, prompt) in self.resume.clone() {
            tokio::spawn(async move {
                use api::driver::SessionDriver;
                if let Err(error) = runner.wake(&session, &prompt, Vec::new()).await {
                    tracing::warn!(%error, %session, "could not re-prompt the Session after the update");
                }
            });
        }
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

/// Tool names the Prompt Store carries a description for: `tool.<name>.description`.
pub fn tool_descriptions(prompts: &PromptStore) -> HashMap<String, String> {
    prompts::PROMPT_IDS
        .iter()
        .filter_map(|id| {
            let name = prompts::tool_name(id)?;
            let text = prompts.text(id)?;
            Some((name.to_string(), text))
        })
        .filter(|(_, text)| !text.trim().is_empty())
        .collect()
}

pub async fn run_mcp(
    root: PathBuf,
    spill: PathBuf,
    config_root: PathBuf,
    global_skills: Option<PathBuf>,
) -> anyhow::Result<()> {
    let descriptions = tool_descriptions(&PromptStore::new(config_root.join("prompts")));
    let toolbox = Toolbox::new(mcp_tool_config(root, spill, &config_root, global_skills))?;
    toolbox.flush_notices().await?;
    Arc::new(McpServer::new(Arc::new(toolbox)).with_descriptions(descriptions))
        .serve_stdio()
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))
}

/// The tool config of an MCP process. The Session's HOME is a harness pi-home, so the runtime env
/// file and the tool keys hang off the config root instead.
pub fn mcp_tool_config(
    root: PathBuf,
    spill: PathBuf,
    config_root: &std::path::Path,
    global_skills: Option<PathBuf>,
) -> ToolConfig {
    ToolConfig {
        env_file: Some(config_root.join("env")),
        tool_keys_file: Some(config_root.join("tool-keys.json")),
        mcp_file: Some(config_root.join("mcp.json")),
        global_skills,
        prompts_dir: Some(config_root.join("prompts")),
        session_url: std::env::var("EMBERLOOP_SESSION_URL").ok(),
        reviewer: std::env::var("EMBERLOOP_REVIEWER").as_deref() == Ok("1"),
        subagent_role: std::env::var("EMBERLOOP_SUBAGENT_ROLE")
            .ok()
            .filter(|v| !v.is_empty()),
        image_input: std::env::var("EMBERLOOP_IMAGE_INPUT").as_deref() != Ok("0"),
        max_call_secs: std::env::var("EMBERLOOP_MAX_CALL_SECS")
            .ok()
            .and_then(|value| value.parse().ok()),
        ctx_spill_enabled: std::env::var("EMBERLOOP_CTX_SPILL").as_deref() == Ok("1"),
        capture_mcp_output: std::env::var("EMBERLOOP_CAPTURE_MCP_OUTPUT").as_deref() == Ok("1"),
        ..ToolConfig::new(root, spill)
    }
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
    // `api/` is never part of the app: an unknown API path is a 404, not the shell. Every other path
    // falls back to the shell, which is how a deep link like `/p/<project>/s/<session>` survives a
    // refresh.
    if path.starts_with("api/") {
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
        Some("webmanifest") => "application/manifest+json",
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

/// announce_update reads the marker a swap left behind and says so once, on the Session that asked
/// for it: the harness resume token in meta.json is what carries the conversation across.
async fn announce_update(
    app_state: &AppState,
    events: &EventBus,
    data_root: &DataRoot,
) -> Option<update::PendingUpdate> {
    let pending = update::take_pending(data_root.root())?;
    tracing::info!(from = %pending.from, to = %pending.to, session = ?pending.session, "binary updated");
    if let Some(ref session_id) = pending.session {
        store_resent_notice(app_state, events, session_id, &pending).await;
    }
    Some(pending)
}

/// The notice is a normal Session event, so it lands in the transcript like any other and survives
/// a reload: the app shows the update where the conversation continues.
async fn store_resent_notice(
    app_state: &AppState,
    events: &EventBus,
    session_id: &str,
    pending: &update::PendingUpdate,
) {
    let stored = match app_state.get_event_log(session_id).await {
        Ok(log) => log
            .append(update_notice(session_id, pending))
            .await
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    match stored {
        Ok(stored) => events.publish(stored),
        Err(error) => tracing::warn!(%error, %session_id, "update notice not stored"),
    }
}

fn update_notice(session_id: &str, pending: &update::PendingUpdate) -> SessionEvent {
    SessionEvent::new(
        session_id.to_string(),
        0,
        EventKind::Notice,
        pending.notice(),
    )
}

/// What `emberloop update` was asked to do.
#[derive(Debug, Clone)]
pub struct UpdateRequest {
    pub manifest_url: String,
    pub session: Option<String>,
    pub check: bool,
    pub rollback: bool,
    pub data_root: PathBuf,
    pub file: Option<PathBuf>,
    pub major: bool,
    pub next_version: bool,
}

/// run_update installs the newest release under `releases/` and switches `current` to it. Inside
/// a Session it only queues the request and tells the model to go idle: the daemon installs,
/// restarts and re-prompts that Session once nothing is running.
pub async fn run_update(request: UpdateRequest) -> anyhow::Result<()> {
    let inside = update::in_session(
        std::env::var_os("EMBERLOOP_SESSION_DIR")
            .map(PathBuf::from)
            .as_deref(),
    );
    // Inside a Session `HOME` is a harness pi-home, so the Session's data root is the install root.
    let root = inside
        .as_ref()
        .map(|found| found.data_root.clone())
        .unwrap_or_else(update::install_root);
    // The running daemon reads its queue from its own data root.
    let queue_root = inside
        .as_ref()
        .map(|found| found.data_root.clone())
        .unwrap_or_else(|| request.data_root.clone());
    if request.next_version {
        let base = update::current_version(&root).unwrap_or_else(|| update::VERSION.to_string());
        println!("{}", update::next_version(&base, request.major));
        return Ok(());
    }
    if request.rollback {
        let version = update::rollback(&root)?;
        println!("rolled back to {version}; restart the daemon to run it");
        return Ok(());
    }
    if let Some(file) = &request.file {
        return install_local_update(&request, &root, &queue_root, file, inside.as_ref());
    }
    let latest = update::fetch(&request.manifest_url).await?;
    if !update::is_newer(&latest.version, update::VERSION) {
        println!("emberloop {} is up to date", update::VERSION);
        return Ok(());
    }
    let artifact = latest.artifact().ok_or_else(|| {
        anyhow::anyhow!(
            "release {} has no artifact for {}",
            latest.version,
            update::target()
        )
    })?;
    if request.check {
        println!(
            "{} -> {} available: {}",
            update::VERSION,
            latest.version,
            artifact.url
        );
        return Ok(());
    }
    let session = request
        .session
        .or_else(|| inside.as_ref().map(|found| found.id.clone()));
    update::write_request(
        &queue_root,
        &update::InstallRequest {
            to: latest.version.clone(),
            artifact: Some(artifact.clone()),
            file: None,
            session,
        },
    )?;
    println!(
        "{}",
        update::installed_message(update::VERSION, &latest.version, inside.is_some())
    );
    Ok(())
}

/// install_local_update queues a local build for the running daemon, which installs it and
/// restarts into it once idle. No manifest and no download: the build names its own version
/// (`EMBERLOOP_BUILD_VERSION`).
fn install_local_update(
    request: &UpdateRequest,
    root: &std::path::Path,
    queue_root: &std::path::Path,
    file: &std::path::Path,
    inside: Option<&update::InSession>,
) -> anyhow::Result<()> {
    let base = update::current_version(root).unwrap_or_else(|| update::VERSION.to_string());
    let session = request
        .session
        .clone()
        .or_else(|| inside.map(|found| found.id.clone()));
    let version = update::queue_local(queue_root, file, session)?;
    println!(
        "{}",
        update::installed_message(&base, &version, inside.is_some())
    );
    Ok(())
}

#[cfg(test)]
mod exe_tests {
    use super::*;

    #[test]
    fn the_installed_app_is_served_with_its_own_media_types() {
        assert_eq!(
            content_type("manifest.webmanifest"),
            "application/manifest+json"
        );
        assert_eq!(content_type("sw.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("icon-192.png"), "image/png");
    }

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

    #[tokio::test]
    async fn claude_catalogue_exposes_the_cli_models_and_their_efforts() {
        use api::models::Source;
        let entries = ClaudeSource.fetch().await.unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            [
                "claude/opus",
                "claude/sonnet",
                "claude/claude-fable-5-2",
                "claude/haiku",
                "claude/fable",
                "claude/claude-haiku-4-5-20251001",
                "claude/claude-sonnet-5",
                "claude/claude-opus-5",
                "claude/claude-fable-5",
                "claude/claude-opus-4-8",
                "claude/claude-opus-4-7",
                "claude/claude-opus-4-6",
                "claude/claude-sonnet-4-6",
            ]
        );
        for entry in entries {
            let context_window = if entry.id == "claude/claude-haiku-4-5-20251001" {
                200_000
            } else {
                1_000_000
            };
            assert_eq!(entry.context_window, context_window);
            let efforts: &[&str] = match entry.id.as_str() {
                "claude/claude-haiku-4-5-20251001" => &[],
                "claude/claude-opus-4-6" | "claude/claude-sonnet-4-6" => {
                    &["low", "medium", "high", "max"]
                }
                _ => &["low", "medium", "high", "xhigh", "max"],
            };
            assert_eq!(entry.efforts, efforts);
        }
    }
}
