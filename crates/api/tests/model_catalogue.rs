#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use api::models::{CatalogueError, ModelCatalogue, ModelEntry, Source};
use api::{build_router, AppState};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use emberloop_core::events::EventBus;
use store::{DataRoot, ProjectsStore, SessionsStore};
use tempfile::tempdir;
use tower::ServiceExt;

struct FakeSource {
    name: &'static str,
    reply: Mutex<Result<Vec<ModelEntry>, CatalogueError>>,
}

impl FakeSource {
    fn new(name: &'static str, reply: Result<Vec<ModelEntry>, CatalogueError>) -> Self {
        Self {
            name,
            reply: Mutex::new(reply),
        }
    }

    fn reply(&self, reply: Result<Vec<ModelEntry>, CatalogueError>) {
        *self.reply.lock().expect("fake source lock") = reply;
    }
}

#[async_trait]
impl Source for FakeSource {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn fetch(&self) -> Result<Vec<ModelEntry>, CatalogueError> {
        self.reply.lock().expect("fake source lock").clone()
    }
}

fn entry(id: &str, label: &str, source: &str) -> ModelEntry {
    ModelEntry {
        id: id.to_string(),
        provider: source.to_string(),
        label: label.to_string(),
        kind: "chat".to_string(),
        efforts: vec!["low".to_string(), "high".to_string()],
        image_input: true,
        context_window: 1_048_576,
        source: source.to_string(),
        stale: false,
        updated_at: 0,
    }
}

fn catalogue_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("models").join("catalogue.json")
}

#[tokio::test]
async fn entries_merge_by_id_across_sources() {
    let dir = tempdir().unwrap();
    let first = Arc::new(FakeSource::new(
        "agy",
        Ok(vec![entry("shared", "First", "agy")]),
    ));
    let second = Arc::new(FakeSource::new(
        "fake",
        Ok(vec![
            entry("shared", "Second", "fake"),
            entry("other", "Other", "fake"),
        ]),
    ));
    let catalogue = ModelCatalogue::new(catalogue_path(&dir), vec![first, second]);

    let merged = catalogue.refresh().await.unwrap();

    assert_eq!(merged.len(), 2);
    assert_eq!(
        merged
            .iter()
            .find(|model| model.id == "shared")
            .unwrap()
            .label,
        "Second"
    );
    assert!(merged.iter().any(|model| model.id == "other"));
}

#[tokio::test]
async fn refresh_persists_an_atomic_snapshot() {
    let dir = tempdir().unwrap();
    let path = catalogue_path(&dir);
    let source = Arc::new(FakeSource::new(
        "agy",
        Ok(vec![entry("gemini-3.8-flash", "Gemini 3.8 Flash", "agy")]),
    ));
    let catalogue = ModelCatalogue::new(path.clone(), vec![source]);

    catalogue.refresh().await.unwrap();

    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    let stored: Vec<ModelEntry> = serde_json::from_str(&raw).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, "gemini-3.8-flash");
    assert!(!path.with_file_name("catalogue.json.tmp").exists());
}

#[tokio::test]
async fn a_failing_source_keeps_last_good_entries_stale() {
    let dir = tempdir().unwrap();
    let source = Arc::new(FakeSource::new(
        "agy",
        Ok(vec![entry("gemini-3.8-flash", "Flash", "agy")]),
    ));
    let catalogue = ModelCatalogue::new(catalogue_path(&dir), vec![source.clone()]);
    let fresh = catalogue.refresh().await.unwrap();
    assert!(!fresh[0].stale);
    assert!(fresh[0].updated_at > 0);

    source.reply(Err(CatalogueError::new("agy is down")));
    let stale = catalogue.refresh().await.unwrap();

    assert_eq!(stale.len(), 1);
    assert!(stale[0].stale);
    assert_eq!(
        catalogue.errors().await.get("agy").map(String::as_str),
        Some("agy is down")
    );
}

#[tokio::test]
async fn restart_serves_the_snapshot_before_the_first_refresh() {
    let dir = tempdir().unwrap();
    let path = catalogue_path(&dir);
    let source = Arc::new(FakeSource::new(
        "agy",
        Ok(vec![entry("gemini-2.5-pro", "Gemini 2.5 Pro", "agy")]),
    ));
    let first = ModelCatalogue::new(path.clone(), vec![source]);
    first.refresh().await.unwrap();

    let offline = Arc::new(FakeSource::new("agy", Err(CatalogueError::new("offline"))));
    let restarted = ModelCatalogue::new(path, vec![offline]);
    let loaded = restarted.entries().await;
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "gemini-2.5-pro");

    let retained = restarted.refresh().await.unwrap();
    assert_eq!(retained.len(), 1);
    assert!(retained[0].stale);
}

async fn app_with(source: Arc<FakeSource>, path: PathBuf) -> axum::Router {
    let data_root = DataRoot::new(path.parent().unwrap().parent().unwrap());
    let projects = ProjectsStore::load(data_root.projects_file())
        .await
        .unwrap();
    let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
    let mut state = AppState::new(data_root, projects, sessions, Arc::new(EventBus::default()));
    state.models = Arc::new(ModelCatalogue::new(path, vec![source]));
    build_router(state)
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn models_endpoints_list_and_force_a_refresh() {
    let dir = tempdir().unwrap();
    let path = catalogue_path(&dir);
    let source = Arc::new(FakeSource::new(
        "agy",
        Ok(vec![entry("gemini-3.8-flash", "Flash", "agy")]),
    ));
    let app = app_with(source.clone(), path).await;

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert!(json(listed).await.as_array().unwrap().is_empty());

    source.reply(Ok(vec![entry("gemini-3.8-flash", "Flash", "agy")]));
    let refreshed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/models/refresh")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refreshed.status(), StatusCode::OK);
    let refreshed = json(refreshed).await;
    assert_eq!(refreshed[0]["id"], "gemini-3.8-flash");
    assert_eq!(refreshed[0]["context_window"], 1_048_576);

    let listed = app
        .oneshot(
            Request::builder()
                .uri("/api/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(json(listed).await[0]["label"], "Flash");
}

#[tokio::test]
async fn a_session_stores_the_picked_model() {
    let dir = tempdir().unwrap();
    let data_root = DataRoot::new(dir.path());
    let projects = ProjectsStore::load(data_root.projects_file())
        .await
        .unwrap();
    let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
    let app = build_router(AppState::new(
        data_root,
        projects,
        sessions,
        Arc::new(EventBus::default()),
    ));

    let project = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"name": "Demo", "root": dir.path()}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let project = json(project).await;
    let project_id = project["id"].as_str().unwrap();

    let created = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "projectId": project_id,
                        "model": "gemini-2.5-pro"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(json(created).await["model"], "gemini-2.5-pro");
}
