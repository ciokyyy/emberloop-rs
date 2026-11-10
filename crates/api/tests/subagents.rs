#![allow(clippy::unwrap_used, clippy::expect_used)]
use api::routes::PromptImageDto;
use api::{build_router, AppState, SessionDriver};
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use emberloop_core::events::{EventBus, EventKind};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use store::{DataRoot, Project, ProjectsStore, SessionMeta, SessionsStore};
use tower::ServiceExt;

#[derive(Default)]
struct Driver {
    prompts: Mutex<Vec<String>>,
    cancelled: Mutex<Vec<String>>,
    sessions: Option<SessionsStore>,
}
#[async_trait]
impl SessionDriver for Driver {
    async fn prompt(&self, _: &str, text: &str, _: Vec<PromptImageDto>) -> Result<(), String> {
        self.prompts.lock().unwrap().push(text.into());
        Ok(())
    }
    async fn wake(&self, _: &str, text: &str, _: Vec<PromptImageDto>) -> Result<(), String> {
        self.prompts.lock().unwrap().push(text.into());
        Ok(())
    }
    async fn cancel(&self, id: &str) -> Result<(), String> {
        self.cancelled.lock().unwrap().push(id.into());
        Ok(())
    }
    async fn connect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    async fn configure_model(&self, id: &str, _: &str, _: Option<&str>) -> Result<(), String> {
        if let Some(sessions) = &self.sessions {
            sessions.get(id).await.ok_or("Session not found")?;
        }
        Ok(())
    }
}
async fn setup(path: &std::path::Path) -> (AppState, Arc<Driver>) {
    let root = DataRoot::new(path.join("data"));
    let projects = ProjectsStore::load(root.projects_file()).await.unwrap();
    projects
        .insert(Project {
            id: "p".into(),
            name: "Project".into(),
            root: path.into(),
        })
        .await
        .unwrap();
    let sessions = SessionsStore::load(root.clone()).await.unwrap();
    sessions
        .insert(SessionMeta {
            id: "parent".into(),
            project_id: "p".into(),
            ..SessionMeta::default()
        })
        .await
        .unwrap();
    std::fs::write(path.join("ticket.md"), "# Required behavior").unwrap();
    std::fs::write(path.join("next.md"), "# Next ticket").unwrap();
    let driver = Arc::new(Driver {
        sessions: Some(sessions.clone()),
        ..Driver::default()
    });
    let state = AppState::new(root, projects, sessions, Arc::new(EventBus::default()))
        .with_driver(driver.clone());
    state.models.refresh().await.unwrap();
    (state, driver)
}
async fn call(state: &AppState, id: &str, value: Value) -> (StatusCode, Value) {
    let response = build_router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{id}/subagents"))
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| json!({"error": String::from_utf8_lossy(&body)})),
    )
}
async fn spawn(state: &AppState, role: &str, ticket: &str) -> String {
    let (status, body) = call(
        state,
        "parent",
        json!({"action":"spawn", "role": role, "ticketPath":ticket, "startingPoint":"abc123"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["id"].as_str().unwrap().into()
}
#[tokio::test]
async fn reviewers_persist_across_tickets_and_each_submit_delivers_including_identical_reports() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = setup(dir.path()).await;
    let id = spawn(&state, "spec", "ticket.md").await;
    let child = state.sessions.get(&id).await.unwrap();
    assert!(
        child.created_at > 1_000_000_000_000,
        "Session timestamps use milliseconds"
    );
    assert!(child.last_used_at.unwrap() >= child.created_at);
    assert_ne!(id, spawn(&state, "standards", "ticket.md").await);
    assert_eq!(id, spawn(&state, "spec", "next.md").await);
    state.steers.queue("parent").set_turn_active(true);
    for _ in 0..3 {
        let (status, _) = call(&state, &id, json!({"action":"submit","status":" done ","report":"Missing test","summary":"Needs\n test"})).await;
        assert_eq!(status, StatusCode::OK);
    }
    let entries = state.steers.queue("parent").entries().await;
    assert_eq!(entries.len(), 3);
    assert_ne!(entries[0].id, entries[1].id);
    assert_ne!(entries[1].id, entries[2].id);
    let report = entries[0].report.as_ref().unwrap();
    assert_eq!(report.ticket_path, "next.md");
    assert_eq!(report.summary, "Needs test");
    let events = state
        .get_event_log("parent")
        .await
        .unwrap()
        .kind_tail(&EventKind::SteerQueued, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].data["report"]["sessionId"], id);
    let (status, results) = call(&state, "parent", json!({"action":"result","id":id})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(results["reports"].as_array().unwrap().len(), 3);
    let sessions = SessionsStore::load(state.data_root.clone()).await.unwrap();
    assert!(sessions.get(&id).await.is_some());
}

#[tokio::test]
async fn errors_notify_parent_without_a_submit_and_stop_keeps_the_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    let id = spawn(&state, "spec", "ticket.md").await;
    state.steers.queue("parent").set_turn_active(true);
    let watcher = api::subagents::watch_errors(state.clone());
    let mut events = state.event_bus.subscribe();
    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            &id,
            1,
            EventKind::AssistantChunk,
            json!({"text":"A normal final answer"}),
        ));
    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            &id,
            2,
            EventKind::Status,
            json!({"state":"idle"}),
        ));
    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            &id,
            1,
            EventKind::Error,
            json!({"message":"provider failed"}),
        ));
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind == EventKind::SteerQueued {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(event.data["report"]["status"], "error");
    assert!(event.data["text"]
        .as_str()
        .unwrap()
        .contains("Tell the user: provider failed"));
    assert_eq!(state.steers.queue("parent").entries().await.len(), 1);
    let (status, _) = call(&state, "parent", json!({"action":"stop", "id":id})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        driver.cancelled.lock().unwrap().as_slice(),
        std::slice::from_ref(&id)
    );
    assert!(state.sessions.get(&id).await.is_some());
    watcher.abort();
}

#[tokio::test]
async fn invalid_submits_do_not_deliver_and_unicode_limits_count_characters() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = setup(dir.path()).await;
    let id = spawn(&state, "spec", "ticket.md").await;
    for body in [
        json!({"action":"submit","status":"pass"}),
        json!({"action":"submit","status":"done","report":"é".repeat(12001)}),
        json!({"action":"submit","status":"done","summary":"🦀".repeat(301)}),
    ] {
        assert_eq!(call(&state, &id, body).await.0, StatusCode::BAD_REQUEST);
    }
    assert!(state.steers.queue("parent").entries().await.is_empty());
    assert_eq!(
        call(&state, "parent", json!({"action":"submit","status":"done"}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(call(&state, &id, json!({"action":"submit","status":"done","report":"é".repeat(12000),"summary":"🦀".repeat(300)})).await.0, StatusCode::OK);
    let entry = state.steers.queue("parent").entries().await.pop().unwrap();
    assert_eq!(entry.state, emberloop_core::steer::SteerState::Delivered);
}

#[tokio::test]
async fn creation_defaults_are_not_reapplied_and_other_parents_cannot_control_reviewers() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = setup(dir.path()).await;
    let id = spawn(&state, "spec", "ticket.md").await;
    state.settings.write().await.spec_reviewer.model = "unknown".into();
    assert_eq!(id, spawn(&state, "spec", "next.md").await);
    assert_eq!(
        state.sessions.get(&id).await.unwrap().model,
        "gemini-3.8-flash"
    );
    state
        .sessions
        .insert(SessionMeta {
            id: "other".into(),
            project_id: "p".into(),
            ..SessionMeta::default()
        })
        .await
        .unwrap();
    assert_eq!(
        call(&state, "other", json!({"action":"result","id":id}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&state, "other", json!({"action":"stop","id":id}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(call(&state, "other", json!({"action":"spawn","role":"spec","ticketPath":"ticket.md","startingPoint":"abc123"})).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn active_assignments_cannot_change_and_new_parents_get_their_own_pair() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = setup(dir.path()).await;
    let id = spawn(&state, "spec", "ticket.md").await;
    state.steers.queue(&id).set_turn_active(true);
    assert_eq!(
        call(
            &state,
            "parent",
            json!({"action":"spawn","role":"spec","ticketPath":"next.md","startingPoint":"abc123"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        state
            .sessions
            .get(&id)
            .await
            .unwrap()
            .reviewer
            .unwrap()
            .ticket_path,
        "ticket.md"
    );
    assert_eq!(call(&state, "parent", json!({"action":"spawn","role":"spec","ticketPath":"../outside.md","startingPoint":"abc123"})).await.0, StatusCode::BAD_REQUEST);
    state
        .sessions
        .insert(SessionMeta {
            id: "other".into(),
            project_id: "p".into(),
            ..SessionMeta::default()
        })
        .await
        .unwrap();
    let (status, other) = call(
        &state,
        "other",
        json!({"action":"spawn","role":"spec","ticketPath":"ticket.md","startingPoint":"abc123"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(other["id"], id);
}

#[tokio::test]
async fn recovery_restores_a_report_saved_before_its_event_without_retrying_review_work() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = setup(dir.path()).await;
    let id = spawn(&state, "spec", "ticket.md").await;
    state
        .steers
        .queue("parent")
        .enqueue_report(emberloop_core::steer::ReviewerReport {
            session_id: id,
            role: "spec".into(),
            ticket_path: "ticket.md".into(),
            status: "done".into(),
            summary: "Saved".into(),
            report: "Report".into(),
        })
        .await
        .unwrap();
    let root = state.data_root.clone();
    let driver = Arc::new(Driver::default());
    let restored = AppState::new(
        root.clone(),
        ProjectsStore::load(root.projects_file()).await.unwrap(),
        SessionsStore::load(root).await.unwrap(),
        Arc::new(EventBus::default()),
    )
    .with_driver(driver.clone());
    api::subagents::recover(&restored).await.unwrap();
    api::subagents::recover(&restored).await.unwrap();
    assert_eq!(driver.prompts.lock().unwrap().len(), 1);
    assert!(driver.prompts.lock().unwrap()[0].contains("Sub-agent report"));
    assert_eq!(
        restored
            .get_event_log("parent")
            .await
            .unwrap()
            .kind_tail(&EventKind::SteerQueued, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}
