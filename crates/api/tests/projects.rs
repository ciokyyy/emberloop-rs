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

async fn patch(router: &axum::Router, id: &str, body: &str) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/projects/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_str(&body(response).await).unwrap()
}

#[tokio::test]
async fn project_tools_patch_validates_keeps_on_omit_and_clears_on_empty() {
    let dir = tempdir().unwrap();
    let router = build_router(state(dir.path()).await);

    let created = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"P","root":"/tmp/p"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = json(created).await;
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["tools"], serde_json::json!([]));

    let patched = patch(
        &router,
        &id,
        r#"{"tools":[{"name":"photocraft","url":"https://box.ts.net/mcp"},{"name":"docs","enabled":false}]}"#,
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);
    assert_eq!(
        json(patched).await["tools"],
        serde_json::json!([
            {"name": "photocraft", "url": "https://box.ts.net/mcp", "enabled": true},
            {"name": "docs", "url": "", "enabled": false}
        ])
    );

    let renamed = patch(&router, &id, r#"{"name":"Renamed"}"#).await;
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed = json(renamed).await;
    assert_eq!(renamed["name"], "Renamed");
    assert_eq!(renamed["tools"].as_array().unwrap().len(), 2);

    let rejected = patch(&router, &id, r#"{"tools":[{"name":"PhotoCraft"}]}"#).await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body(rejected).await,
        "project tools: name \"PhotoCraft\" is invalid"
    );

    let cleared = patch(&router, &id, r#"{"tools":[]}"#).await;
    assert_eq!(cleared.status(), StatusCode::OK);
    assert_eq!(json(cleared).await["tools"], serde_json::json!([]));
    let reloaded = ProjectsStore::load(DataRoot::new(dir.path()).projects_file())
        .await
        .unwrap();
    assert!(reloaded.get(&id).await.unwrap().tools.is_empty());
}
