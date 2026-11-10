#![allow(clippy::unwrap_used, clippy::expect_used)]
use api::state::Settings;
use api::{build_router, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use emberloop_core::events::EventBus;
use std::sync::Arc;
use store::{DataRoot, ProjectsStore, SessionsStore};
use tempfile::tempdir;
use tower::ServiceExt;

#[test]
fn skills_review_interval_defaults_to_five_and_stored_settings_without_it_load() {
    assert_eq!(Settings::default().skills_review_interval, 5);
    let stored: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(stored.skills_review_interval, 5);
}

async fn state(root: &DataRoot) -> AppState {
    let projects = ProjectsStore::load(root.projects_file()).await.unwrap();
    let sessions = SessionsStore::load(root.clone()).await.unwrap();
    AppState::new(
        root.clone(),
        projects,
        sessions,
        Arc::new(EventBus::default()),
    )
}

#[tokio::test]
async fn reviewer_models_survive_a_daemon_restart() {
    let dir = tempdir().unwrap();
    let root = DataRoot::new(dir.path());
    let first = state(&root).await;
    first.models.refresh().await.unwrap();
    let app = build_router(first);

    let mut settings = Settings::default();
    settings.spec_reviewer.model = "claude-opus-5-5".into();
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&settings).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(dir.path().join("settings.json").is_file());

    let restarted = state(&root).await;
    let loaded = restarted.settings.read().await.clone();
    assert_eq!(loaded.spec_reviewer.model, "claude-opus-5-5");
    assert_eq!(
        loaded.standards_reviewer.model,
        Settings::default().standards_reviewer.model
    );
}
