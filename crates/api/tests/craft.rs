#![allow(clippy::unwrap_used, clippy::expect_used)]
use api::{build_router, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use emberloop_core::events::EventBus;
use std::path::Path;
use std::sync::Arc;
use store::{DataRoot, ProjectsStore, SessionsStore};
use tempfile::tempdir;
use tower::ServiceExt;

async fn state(dir: &Path) -> AppState {
    let root = DataRoot::new(dir);
    let projects = ProjectsStore::load(root.projects_file()).await.unwrap();
    let sessions = SessionsStore::load(root.clone()).await.unwrap();
    AppState::new(root, projects, sessions, Arc::new(EventBus::default()))
}

#[tokio::test]
async fn craft_lists_figcraft_first_then_every_verified_app() {
    let dir = tempdir().unwrap();
    let router = build_router(state(dir.path()).await);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/craft")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let apps: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let apps = apps.as_array().unwrap();
    assert_eq!(apps.len(), tools::craft::APPS.len() + 1);
    assert_eq!(apps[0]["name"], "figcraft");
    assert!(apps.iter().any(|app| app["name"] == "photocraft"));
    assert!(apps.iter().all(
        |app| app["label"].as_str().is_some_and(|label| !label.is_empty())
            && app["installed"].is_boolean()
    ));
}
