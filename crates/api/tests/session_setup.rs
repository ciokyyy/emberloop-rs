#![allow(clippy::unwrap_used, clippy::expect_used)]
use api::routes::PromptImageDto;
use api::{build_router, AppState, SessionDriver};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use emberloop_core::events::EventBus;
use std::sync::{Arc, Mutex};
use store::{DataRoot, ProjectsStore, SessionMeta, SessionsStore};
use tower::ServiceExt;

#[derive(Default)]
struct Driver {
    configured: Mutex<Vec<String>>,
    images: Mutex<Vec<usize>>,
    fail: bool,
}
#[async_trait]
impl SessionDriver for Driver {
    async fn prompt(&self, _: &str, _: &str, images: Vec<PromptImageDto>) -> Result<(), String> {
        self.images.lock().unwrap().push(images.len());
        Ok(())
    }
    async fn cancel(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    async fn wake(&self, _: &str, _: &str, _: Vec<PromptImageDto>) -> Result<(), String> {
        Ok(())
    }
    async fn connect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    async fn configure_model(
        &self,
        _: &str,
        model: &str,
        effort: Option<&str>,
    ) -> Result<(), String> {
        if self.fail {
            return Err("refused".into());
        }
        self.configured
            .lock()
            .unwrap()
            .push(format!("{model}:{}", effort.unwrap_or("")));
        Ok(())
    }
}

async fn state(dir: &std::path::Path, driver: Arc<Driver>) -> AppState {
    let root = DataRoot::new(dir);
    let projects = ProjectsStore::load(root.projects_file()).await.unwrap();
    let sessions = SessionsStore::load(root.clone()).await.unwrap();
    sessions
        .insert(SessionMeta {
            id: "s1".into(),
            model: "gemini-3.8-flash".into(),
            ..SessionMeta::default()
        })
        .await
        .unwrap();
    let state =
        AppState::new(root, projects, sessions, Arc::new(EventBus::default())).with_driver(driver);
    state.models.refresh().await.unwrap();
    state
}
fn req(method: &str, path: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn model_and_effort_are_validated_applied_and_persisted_together() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(Driver::default());
    let state = state(dir.path(), driver.clone()).await;
    let app = build_router(state.clone());
    let response = app
        .oneshot(req(
            "PATCH",
            "/api/sessions/s1",
            serde_json::json!({"model":"claude-opus-5-5","thoughtLevel":"high"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state.sessions.get("s1").await.unwrap().model,
        "claude-opus-5-5"
    );
    assert_eq!(
        driver.configured.lock().unwrap().as_slice(),
        ["claude-opus-5-5:high"]
    );
}

#[tokio::test]
async fn invalid_efforts_busy_sessions_and_driver_failure_do_not_change_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(
        dir.path(),
        Arc::new(Driver {
            fail: true,
            ..Driver::default()
        }),
    )
    .await;
    let app = build_router(state.clone());
    for (body, status) in [
        (
            serde_json::json!({"model":"gpt-oss-120b","thoughtLevel":"max"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            serde_json::json!({"model":"missing"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            serde_json::json!({"model":"gpt-oss-120b","thoughtLevel":"medium"}),
            StatusCode::CONFLICT,
        ),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(req("PATCH", "/api/sessions/s1", body))
                .await
                .unwrap()
                .status(),
            status
        );
    }
    state.steers.queue("s1").set_turn_active(true);
    assert_eq!(
        app.oneshot(req(
            "PATCH",
            "/api/sessions/s1",
            serde_json::json!({"model":"gpt-oss-120b"})
        ))
        .await
        .unwrap()
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        state.sessions.get("s1").await.unwrap().model,
        "gemini-3.8-flash"
    );
}

#[tokio::test]
async fn images_over_the_default_two_megabyte_body_limit_reach_the_driver() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(Driver::default());
    let state = state(dir.path(), driver.clone()).await;
    let app = build_router(state);
    let response = app.oneshot(req("POST", "/api/sessions/s1/prompt", serde_json::json!({"text":"", "images":[{"mimeType":"image/png", "dataBase64":"A".repeat(3*1024*1024)}]}))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(driver.images.lock().unwrap().as_slice(), [1]);
}

#[tokio::test]
async fn images_sent_during_a_turn_are_queued_as_a_steer() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(Driver::default());
    let state = state(dir.path(), driver.clone()).await;
    state.steers.queue("s1").set_turn_active(true);
    let app = build_router(state.clone());
    let response = app.oneshot(req("POST", "/api/sessions/s1/prompt", serde_json::json!({"text":"see image", "images":[{"mimeType":"image/png", "dataBase64":"AAAA"}]}))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(driver.images.lock().unwrap().is_empty());
    let entries = state.steers.queue("s1").entries().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].images.len(), 1);
    assert_eq!(entries[0].images[0].data_base64, "AAAA");
    let log = state.get_event_log("s1").await.unwrap();
    let events = log.replay(None).await.unwrap();
    let queued = events
        .iter()
        .find(|event| event.kind == emberloop_core::events::EventKind::SteerQueued)
        .unwrap();
    assert_eq!(queued.data["imagesCount"], 1);
}

#[tokio::test]
async fn an_explicit_empty_effort_clears_a_previously_saved_effort() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path(), Arc::new(Driver::default())).await;
    state
        .sessions
        .update_thought_level("s1", Some("high".into()))
        .await
        .unwrap();
    let app = build_router(state.clone());
    let response = app
        .oneshot(req(
            "PATCH",
            "/api/sessions/s1",
            serde_json::json!({"thoughtLevel":""}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.sessions.get("s1").await.unwrap().thought_level, None);
}

#[tokio::test]
async fn new_sessions_use_the_default_model_and_reject_unknown_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path(), Arc::new(Driver::default())).await;
    let project = state
        .projects
        .insert(store::Project {
            id: "p1".into(),
            name: "project".into(),
            root: dir.path().to_path_buf(),
        })
        .await
        .unwrap();
    let app = build_router(state);
    let response = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/settings",
            serde_json::json!({"default_model":"claude-opus-5-5"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/sessions",
            serde_json::json!({"projectId":project.id}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let session: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(session["model"], "claude-opus-5-5");
    let response = app
        .oneshot(req(
            "PUT",
            "/api/settings",
            serde_json::json!({"default_model":"missing"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

struct Listed(&'static str, &'static [&'static str]);

#[async_trait]
impl api::models::Source for Listed {
    fn name(&self) -> &'static str {
        self.0
    }
    async fn fetch(&self) -> Result<Vec<api::models::ModelEntry>, api::models::CatalogueError> {
        Ok(self
            .1
            .iter()
            .map(|id| api::models::ModelEntry {
                id: (*id).into(),
                provider: self.0.into(),
                label: (*id).into(),
                kind: "chat".into(),
                efforts: Vec::new(),
                image_input: false,
                context_window: 1000,
                source: String::new(),
                stale: false,
                updated_at: 0,
            })
            .collect())
    }
}

#[tokio::test]
async fn a_model_from_another_harness_drops_the_old_resume_token_and_the_same_harness_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path(), Arc::new(Driver::default())).await;
    let sources: Vec<Arc<dyn api::models::Source>> = vec![
        Arc::new(Listed("agy", &["gemini-3.8-flash", "gemini-4"])),
        Arc::new(Listed("claude", &["claude/haiku"])),
    ];
    let models = Arc::new(api::models::ModelCatalogue::new(
        dir.path().join("catalogue.json"),
        sources,
    ));
    models.refresh().await.unwrap();
    let state = state.with_models(models);
    let mut meta = state.sessions.get("s1").await.unwrap();
    meta.resume_token = Some("agy-conversation".into());
    state.sessions.save(&meta).await.unwrap();
    let app = build_router(state.clone());
    for (model, token) in [
        ("gemini-4", Some("agy-conversation")),
        ("claude/haiku", None),
    ] {
        let response = app
            .clone()
            .oneshot(req(
                "PATCH",
                "/api/sessions/s1",
                serde_json::json!({ "model": model }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let saved = state.sessions.get("s1").await.unwrap();
        assert_eq!(saved.resume_token.as_deref(), token, "{model}");
    }
}
