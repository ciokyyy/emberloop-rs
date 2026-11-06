use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use utoipa::ToSchema;

const REFRESH_INTERVAL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ModelEntry {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub kind: String,
    pub efforts: Vec<String>,
    pub image_input: bool,
    pub context_window: usize,
    pub source: String,
    pub stale: bool,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueError {
    message: String,
}

impl CatalogueError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CatalogueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CatalogueError {}

#[async_trait]
pub trait Source: Send + Sync {
    fn name(&self) -> &'static str;
    async fn fetch(&self) -> Result<Vec<ModelEntry>, CatalogueError>;
}

pub struct ModelCatalogue {
    path: PathBuf,
    sources: Vec<Arc<dyn Source>>,
    last_good: RwLock<HashMap<String, Vec<ModelEntry>>>,
    errors: RwLock<HashMap<String, String>>,
    refresh_lock: Mutex<()>,
}

impl ModelCatalogue {
    pub fn new(path: PathBuf, sources: Vec<Arc<dyn Source>>) -> Self {
        let mut last_good = HashMap::new();
        for entry in load_snapshot(&path) {
            last_good
                .entry(entry.source.clone())
                .or_insert_with(Vec::new)
                .push(entry);
        }
        Self {
            path,
            sources,
            last_good: RwLock::new(last_good),
            errors: RwLock::new(HashMap::new()),
            refresh_lock: Mutex::new(()),
        }
    }

    pub async fn entries(&self) -> Vec<ModelEntry> {
        self.merge().await
    }

    /// The context window a model id is known to have; `None` when no source
    /// lists it or the window is unknown.
    pub async fn window(&self, model: &str) -> Option<usize> {
        self.merge()
            .await
            .into_iter()
            .find(|entry| entry.id == model)
            .map(|entry| entry.context_window)
            .filter(|window| *window > 0)
    }

    pub async fn errors(&self) -> HashMap<String, String> {
        self.errors.read().await.clone()
    }

    pub async fn refresh(&self) -> Result<Vec<ModelEntry>, CatalogueError> {
        let _guard = self.refresh_lock.lock().await;
        let now = unix_now();
        for source in &self.sources {
            let name = source.name().to_string();
            match source.fetch().await {
                Ok(mut entries) => {
                    for entry in &mut entries {
                        entry.source = name.clone();
                        entry.stale = false;
                        entry.updated_at = now;
                    }
                    self.last_good.write().await.insert(name.clone(), entries);
                    self.errors.write().await.remove(&name);
                }
                Err(error) => {
                    let mut retained = self
                        .last_good
                        .read()
                        .await
                        .get(&name)
                        .cloned()
                        .unwrap_or_default();
                    for entry in &mut retained {
                        entry.stale = true;
                        entry.updated_at = now;
                    }
                    if !retained.is_empty() {
                        self.last_good.write().await.insert(name.clone(), retained);
                    }
                    self.errors.write().await.insert(name, error.to_string());
                }
            }
        }
        let merged = self.merge().await;
        persist(&self.path, &merged).await?;
        Ok(merged)
    }

    pub fn spawn_refresher(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let catalogue = Arc::clone(self);
        tokio::spawn(async move {
            let _ = catalogue.refresh().await;
            let mut interval = tokio::time::interval(REFRESH_INTERVAL);
            interval.tick().await;
            loop {
                interval.tick().await;
                let _ = catalogue.refresh().await;
            }
        })
    }

    async fn merge(&self) -> Vec<ModelEntry> {
        let last_good = self.last_good.read().await;
        let mut merged: Vec<ModelEntry> = Vec::new();
        for source in &self.sources {
            let Some(entries) = last_good.get(source.name()) else {
                continue;
            };
            for entry in entries {
                match merged.iter().position(|model| model.id == entry.id) {
                    Some(index) => merged[index] = entry.clone(),
                    None => merged.push(entry.clone()),
                }
            }
        }
        merged
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn load_snapshot(path: &Path) -> Vec<ModelEntry> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

async fn persist(path: &Path, entries: &[ModelEntry]) -> Result<(), CatalogueError> {
    let dir = path
        .parent()
        .ok_or_else(|| CatalogueError::new("catalogue path has no parent"))?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|error| CatalogueError::new(error.to_string()))?;
    let json = serde_json::to_vec_pretty(entries)
        .map_err(|error| CatalogueError::new(error.to_string()))?;
    let browser = path.with_extension("json.tmp");
    tokio::fs::write(&browser, json)
        .await
        .map_err(|error| CatalogueError::new(error.to_string()))?;
    tokio::fs::rename(&browser, path)
        .await
        .map_err(|error| CatalogueError::new(error.to_string()))
}

pub struct AgySource;

#[async_trait]
impl Source for AgySource {
    fn name(&self) -> &'static str {
        "agy"
    }

    async fn fetch(&self) -> Result<Vec<ModelEntry>, CatalogueError> {
        Ok(agy_models())
    }
}

fn agy_models() -> Vec<ModelEntry> {
    const ALL: &[&str] = &["low", "medium", "high"];
    [
        ("gemini-4-argon", "Gemini 4 Argon", ALL),
        ("gemini-3.8-flash", "Gemini 3.8 Flash", ALL),
        ("claude-opus-5-5", "Claude Opus 5.5", ALL),
        ("claude-sonnet-5-5", "Claude Sonnet 5.5", ALL),
        ("gpt-oss-120b", "GPT-OSS 120B", &["medium"]),
    ]
    .into_iter()
    .map(|(id, label, efforts)| ModelEntry {
        id: id.to_string(),
        provider: "agy".to_string(),
        label: label.to_string(),
        kind: "chat".to_string(),
        efforts: efforts.iter().map(ToString::to_string).collect(),
        image_input: true,
        context_window: 1_048_576,
        source: "agy".to_string(),
        stale: false,
        updated_at: 0,
    })
    .collect()
}
