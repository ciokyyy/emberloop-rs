use crate::driver::{NoopDriver, SessionDriver};
use crate::models::ModelCatalogue;
use crate::providers::ProviderLogin;
use emberloop_core::events::EventBus;
use emberloop_core::steer::SteerQueues;
use prompts::PromptStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{DataRoot, EventLog, ProjectsStore, SessionsStore};
use tokio::sync::RwLock;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct Settings {
    pub title_generation: bool,
    pub title_model: String,
    pub spec_reviewer: FallbackLadder,
    pub standards_reviewer: FallbackLadder,
    pub subagent_tiers: SubagentTiers,
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
    /// Background skill review starts after this many tool-calling rounds since the last `skill_manage`; 0 turns it off.
    pub skills_review_interval: u32,
    /// Whether oversized tool output spills to `ctx:` handles and CCR replacement runs.
    pub ctx_spill_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            title_generation: true,
            title_model: "openrouter/openrouter/free".into(),
            default_model: "gemini-3.8-flash".into(),
            spec_reviewer: FallbackLadder::default(),
            standards_reviewer: FallbackLadder::default(),
            subagent_tiers: SubagentTiers::default(),
            proxy_compression: true,
            ponytail_default_level: "full".to_string(),
            todo_reminders: true,
            todo_reminders_max: 3,
            todo_auto_expand: true,
            todo_hide_when_done: true,
            max_context_limit: 20,
            min_pressure_benefit_tokens: 0,
            emergency_threshold_percent: 25,
            preserve_recent_messages: 3,
            protected_tools: Vec::new(),
            inject_tool: true,
            inject_nudge: true,
            tier2_trigger: 1000,
            tier3_trigger: 2000,
            skills_review_interval: 5,
            ctx_spill_enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct ModelRung {
    pub model: String,
    pub reasoning_effort: Option<String>,
}

impl Default for ModelRung {
    fn default() -> Self {
        Self {
            model: "gemini-3.8-flash".into(),
            reasoning_effort: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = Vec<ModelRung>)]
pub struct FallbackLadder(pub Vec<ModelRung>);

impl Default for FallbackLadder {
    fn default() -> Self {
        Self(vec![ModelRung::default()])
    }
}

impl FallbackLadder {
    pub fn single(model: impl Into<String>, reasoning_effort: Option<String>) -> Self {
        Self(vec![ModelRung {
            model: model.into(),
            reasoning_effort,
        }])
    }

    pub fn rungs(&self) -> &[ModelRung] {
        &self.0
    }

    pub fn primary(&self) -> Option<&ModelRung> {
        self.0.first()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for FallbackLadder {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Rungs(Vec<ModelRung>),
            Single(ModelRung),
        }
        match Wire::deserialize(deserializer)? {
            Wire::Rungs(rungs) => Ok(Self(rungs)),
            Wire::Single(rung) => Ok(Self(vec![rung])),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct SubagentTiers {
    pub smol: FallbackLadder,
    pub mid: FallbackLadder,
    pub top: FallbackLadder,
}

/// `settings.json` at the data root: an update to the daemon keeps every choice.
pub fn save_settings(root: &Path, settings: &Settings) -> Result<(), String> {
    let file = settings_file(root);
    let body = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    let temp = file.with_extension("json.tmp");
    std::fs::write(&temp, body)
        .and_then(|()| std::fs::rename(&temp, &file))
        .map_err(|error| format!("cannot save {}: {error}", file.display()))
}

fn settings_file(root: &Path) -> PathBuf {
    root.join("settings.json")
}

fn load_settings(root: &Path) -> Settings {
    std::fs::read(settings_file(root))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
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
    /// The global skills root the skill pages read; `None` lists no global skills.
    pub global_skills: Option<PathBuf>,
    pub designs: Arc<crate::design::Designs>,
    pub subagent_gate: Arc<tokio::sync::Mutex<()>>,
    pub(crate) subagent_runs:
        Arc<tokio::sync::Mutex<HashMap<String, crate::subagent_recovery::SubagentRun>>>,
    pub(crate) subagent_retry_backoffs: [std::time::Duration; 2],
    /// Web Push: the VAPID keypair and the browsers that subscribed.
    pub push: Arc<crate::push::Push>,
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
            Vec::new(),
        ));
        let steers = SteerQueues::with_dir(data_root.sessions_dir());
        let settings = load_settings(data_root.root());
        let push = Arc::new(crate::push::Push::load(data_root.root()));
        Self {
            data_root,
            projects,
            sessions,
            event_bus,
            prompts,
            settings: Arc::new(RwLock::new(settings)),
            event_logs: Arc::new(RwLock::new(HashMap::new())),
            models,
            steers,
            driver: Arc::new(NoopDriver),
            provider_login: None,
            tool_keys_file: None,
            global_skills: None,
            groq: None,
            designs: Arc::new(crate::design::Designs::default()),
            subagent_gate: Arc::new(tokio::sync::Mutex::new(())),
            subagent_runs: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            subagent_retry_backoffs: [
                std::time::Duration::from_secs(2),
                std::time::Duration::from_secs(5),
            ],
            push,
        }
    }

    pub fn with_figcraft(mut self, config: PathBuf) -> Self {
        self.designs = Arc::new(crate::design::Designs::new(config));
        self
    }

    pub fn with_global_skills(mut self, root: impl Into<PathBuf>) -> Self {
        self.global_skills = Some(root.into());
        self
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

    pub fn with_subagent_retry_backoffs(mut self, backoffs: [std::time::Duration; 2]) -> Self {
        self.subagent_retry_backoffs = backoffs;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn app_state_defaults_subagent_retry_backoffs_to_two_and_five_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::new(dir.path().to_path_buf());
        let projects = ProjectsStore::load(root.projects_file()).await.unwrap();
        let sessions = SessionsStore::load(root.clone()).await.unwrap();
        let state = AppState::new(root, projects, sessions, Arc::new(EventBus::default()));
        assert_eq!(
            state.subagent_retry_backoffs,
            [
                std::time::Duration::from_secs(2),
                std::time::Duration::from_secs(5),
            ]
        );
    }
}
