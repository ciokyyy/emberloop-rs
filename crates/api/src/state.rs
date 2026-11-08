use crate::driver::{NoopDriver, SessionDriver};
use crate::models::{AgySource, ModelCatalogue};
use crate::providers::ProviderLogin;
use emberloop_core::events::EventBus;
use emberloop_core::steer::SteerQueues;
use prompts::PromptStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use store::{DataRoot, EventLog, ProjectsStore, SessionsStore};
use tokio::sync::RwLock;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct Settings {
    pub title_generation: bool,
    pub title_model: String,
    pub default_model: String,
    pub proxy_compression: bool,
    pub ponytail_default_level: String,
    pub todo_reminders: bool,
    pub todo_reminders_max: u32,
    pub todo_auto_expand: bool,
    pub todo_hide_when_done: bool,
    /// Reference `compress.maxContextLimit`, a percent of the window.
    pub max_context_limit: u32,
    /// Reference `compress.minPressureBenefitTokens`; 0 means `max(5000, window / 100)`.
    pub min_pressure_benefit_tokens: u32,
    /// Reference `compress.emergencyThresholdPercent`.
    pub emergency_threshold_percent: u32,
    /// Reference `compress.preserveRecentMessages`.
    pub preserve_recent_messages: u32,
    /// Reference `compress.protectedTools`; results of these tools never fold.
    pub protected_tools: Vec<String>,
    /// Reference `compress.injectTool`.
    pub inject_tool: bool,
    /// Reference `compress.injectNudge`.
    pub inject_nudge: bool,
    /// Reference `compress.tiers.tier2Trigger`.
    pub tier2_trigger: u32,
    /// Reference `compress.tiers.tier3Trigger`.
    pub tier3_trigger: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            title_generation: true,
            title_model: "openrouter/openrouter/free".into(),
            default_model: "gemini-3.8-flash".into(),
            proxy_compression: true,
            ponytail_default_level: "full".to_string(),
            todo_reminders: true,
            todo_reminders_max: 3,
            todo_auto_expand: true,
            todo_hide_when_done: true,
            max_context_limit: 75,
            min_pressure_benefit_tokens: 0,
            emergency_threshold_percent: 95,
            preserve_recent_messages: 3,
            protected_tools: Vec::new(),
            inject_tool: true,
            inject_nudge: true,
            tier2_trigger: 1000,
            tier3_trigger: 2000,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub data_root: DataRoot,
    pub projects: ProjectsStore,
    pub sessions: SessionsStore,
    pub event_bus: Arc<EventBus>,
    pub prompts: Arc<PromptStore>,
    pub settings: Arc<RwLock<Settings>>,
    pub event_logs: Arc<RwLock<HashMap<String, EventLog>>>,
    /// Groq key file and transcription URL for `/api/transcribe`; `None` answers 409.
    pub groq: Option<(PathBuf, String)>,
    pub models: Arc<ModelCatalogue>,
    pub steers: SteerQueues,
    pub driver: Arc<dyn SessionDriver>,
    pub provider_login: Option<Arc<dyn ProviderLogin>>,
    /// The file Settings writes tool keys into; `None` disables that screen.
    pub tool_keys_file: Option<PathBuf>,
}

impl AppState {
    pub fn new(
        data_root: DataRoot,
        projects: ProjectsStore,
        sessions: SessionsStore,
        event_bus: Arc<EventBus>,
    ) -> Self {
        let prompts = Arc::new(PromptStore::new(data_root.root().join("prompts")));
        Self::with_prompts(data_root, projects, sessions, event_bus, prompts)
    }

    pub fn with_prompts(
        data_root: DataRoot,
        projects: ProjectsStore,
        sessions: SessionsStore,
        event_bus: Arc<EventBus>,
        prompts: Arc<PromptStore>,
    ) -> Self {
        let models = Arc::new(ModelCatalogue::new(
            data_root.model_catalogue_file(),
            vec![Arc::new(AgySource)],
        ));
        let steers = SteerQueues::with_dir(data_root.sessions_dir());
        Self {
            data_root,
            projects,
            sessions,
            event_bus,
            prompts,
            settings: Arc::new(RwLock::new(Settings::default())),
            event_logs: Arc::new(RwLock::new(HashMap::new())),
            models,
            steers,
            driver: Arc::new(NoopDriver),
            provider_login: None,
            tool_keys_file: None,
            groq: None,
        }
    }

    pub fn with_groq_keys(mut self, keys_file: impl Into<PathBuf>, url: impl Into<String>) -> Self {
        self.groq = Some((keys_file.into(), url.into()));
        self
    }

    pub fn with_tool_keys_file(mut self, tool_keys_file: impl Into<PathBuf>) -> Self {
        self.tool_keys_file = Some(tool_keys_file.into());
        self
    }

    pub fn with_driver(mut self, driver: Arc<dyn SessionDriver>) -> Self {
        self.driver = driver;
        self
    }

    pub fn with_steers(mut self, steers: SteerQueues) -> Self {
        self.steers = steers;
        self
    }

    pub fn with_provider_login(mut self, login: Arc<dyn ProviderLogin>) -> Self {
        self.provider_login = Some(login);
        self
    }

    pub fn with_models(mut self, models: Arc<ModelCatalogue>) -> Self {
        self.models = models;
        self
    }

    pub async fn get_event_log(&self, session_id: &str) -> Result<EventLog, std::io::Error> {
        let mut map = self.event_logs.write().await;
        if let Some(log) = map.get(session_id) {
            return Ok(log.clone());
        }
        let path = self.data_root.session_events_dir(session_id);
        let log = EventLog::open(&path).await.map_err(|e| match e {
            store::event_log::EventLogError::Io(io_err) => io_err,
            store::event_log::EventLogError::Serde(s) => {
                std::io::Error::new(std::io::ErrorKind::InvalidData, s)
            }
        })?;
        map.insert(session_id.to_string(), log.clone());
        Ok(log)
    }
}
