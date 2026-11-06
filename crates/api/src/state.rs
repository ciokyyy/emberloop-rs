use crate::driver::{NoopDriver, SessionDriver};
use crate::models::{AgySource, ModelCatalogue};
use crate::providers::ProviderLogin;
use emberloop_core::events::EventBus;
use emberloop_core::steer::SteerQueues;
use prompts::PromptStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use store::{DataRoot, EventLog, ProjectsStore, SessionsStore};
use tokio::sync::RwLock;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Settings {
    pub proxy_compression: bool,
    pub ponytail_default_level: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            proxy_compression: true,
            ponytail_default_level: "full".to_string(),
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
    pub models: Arc<ModelCatalogue>,
    pub steers: SteerQueues,
    pub driver: Arc<dyn SessionDriver>,
    pub provider_login: Option<Arc<dyn ProviderLogin>>,
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
        }
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
