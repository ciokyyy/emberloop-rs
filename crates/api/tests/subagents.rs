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
    configured: Mutex<Vec<(String, String, Option<String>)>>,
    outcomes: Mutex<std::collections::VecDeque<Result<(), String>>>,
    sessions: Option<SessionsStore>,
}
#[async_trait]
impl SessionDriver for Driver {
    async fn prompt(&self, _: &str, text: &str, _: Vec<PromptImageDto>) -> Result<(), String> {
        self.prompts.lock().unwrap().push(text.into());
        if let Some(next) = self.outcomes.lock().unwrap().pop_front() {
            return next;
        }
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
    async fn configure_model(
        &self,
        id: &str,
        model: &str,
        effort: Option<&str>,
    ) -> Result<(), String> {
        if let Some(sessions) = &self.sessions {
            sessions.get(id).await.ok_or("Session not found")?;
        }
        self.configured
            .lock()
            .unwrap()
            .push((id.into(), model.into(), effort.map(Into::into)));
        Ok(())
    }
}

struct Listed;

#[async_trait]
impl api::models::Source for Listed {
    fn name(&self) -> &'static str {
        "agy"
    }
    async fn fetch(&self) -> Result<Vec<api::models::ModelEntry>, api::models::CatalogueError> {
        Ok(["gemini-3.8-flash", "claude-opus-5-5"]
            .into_iter()
            .map(|id| api::models::ModelEntry {
                id: id.into(),
                provider: "agy".into(),
                label: id.into(),
                kind: "chat".into(),
                efforts: vec!["low".into(), "medium".into(), "high".into()],
                image_input: true,
                context_window: 1_048_576,
                source: "agy".into(),
                stale: false,
                updated_at: 0,
            })
            .collect())
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
    let models = Arc::new(api::models::ModelCatalogue::new(
        root.model_catalogue_file(),
        vec![Arc::new(Listed)],
    ));
    models.refresh().await.unwrap();
    let state = AppState::new(root, projects, sessions, Arc::new(EventBus::default()))
        .with_driver(driver.clone())
        .with_models(models);
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
    state.steers.queue("parent").set_turn_active(true).await;
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
    state.steers.queue("parent").set_turn_active(true).await;
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
        json!({"action":"submit","status":"done","summary":"🦀".repeat(1001)}),
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
    assert_eq!(call(&state, &id, json!({"action":"submit","status":"done","report":"é".repeat(12000),"summary":"🦀".repeat(1000)})).await.0, StatusCode::OK);
    let entry = state.steers.queue("parent").entries().await.pop().unwrap();
    assert_eq!(entry.state, emberloop_core::steer::SteerState::Delivered);
}

#[tokio::test]
async fn creation_defaults_are_not_reapplied_and_other_parents_cannot_control_reviewers() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = setup(dir.path()).await;
    let id = spawn(&state, "spec", "ticket.md").await;
    state.settings.write().await.spec_reviewer =
        api::state::FallbackLadder::single("unknown", None);
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
    state.steers.queue(&id).set_turn_active(true).await;
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
        .enqueue_report(emberloop_core::steer::SubagentReport {
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

#[tokio::test]
async fn worker_spawn_creates_fresh_sessions_and_steer_reuses_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;

    // Empty text or reviewer-only fields are rejected for workers.
    assert_eq!(
        call(
            &state,
            "parent",
            json!({"action":"spawn","role":"worker","text":"   "})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &state,
            "parent",
            json!({"action":"spawn","role":"worker","text":"do x","ticketPath":"ticket.md"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &state,
            "parent",
            json!({"action":"spawn","role":"worker","text":"do x","startingPoint":"abc123"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    // Two spawns create two distinct fresh Sessions.
    let (s1, b1) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"task one"}),
    )
    .await;
    assert_eq!(s1, StatusCode::OK, "{b1}");
    assert_eq!(b1["role"], "worker");
    assert_eq!(b1["tier"], "mid");
    let id1 = b1["id"].as_str().unwrap().to_string();

    let (s2, b2) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"task two","title":"Custom Worker"}),
    )
    .await;
    assert_eq!(s2, StatusCode::OK, "{b2}");
    let id2 = b2["id"].as_str().unwrap().to_string();
    assert_ne!(id1, id2);
    assert_eq!(
        state.sessions.get(&id1).await.unwrap().title,
        "Worker (mid)"
    );
    assert_eq!(
        state.sessions.get(&id2).await.unwrap().title,
        "Custom Worker"
    );

    // Steer reuses existing Session and rejects ticketPath/startingPoint.
    assert_eq!(
        call(
            &state,
            "parent",
            json!({"action":"steer","id":id1,"text":"more","ticketPath":"ticket.md"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (st, sb) = call(
        &state,
        "parent",
        json!({"action":"steer","id":id1,"text":"follow up"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{sb}");
    assert_eq!(driver.prompts.lock().unwrap()[2], "follow up");

    // Steering an active worker queues a mid-turn steer instead of prompting directly.
    state.steers.queue(&id1).set_turn_active(true).await;
    let (qt, qb) = call(
        &state,
        "parent",
        json!({"action":"steer","id":id1,"text":"mid-turn note"}),
    )
    .await;
    assert_eq!(qt, StatusCode::OK, "{qb}");
    assert_eq!(driver.prompts.lock().unwrap().len(), 3);
    assert_eq!(state.steers.queue(&id1).entries().await.len(), 1);
    state.steers.queue(&id1).set_turn_active(false).await;
    // 1-level hierarchy: worker cannot spawn, steer, stop, or result.
    for blocked in [
        json!({"action":"spawn","role":"worker","text":"nested"}),
        json!({"action":"steer","id":id2,"text":"nested"}),
        json!({"action":"stop","id":id2}),
        json!({"action":"result","id":id2}),
    ] {
        assert_eq!(call(&state, &id1, blocked).await.0, StatusCode::BAD_REQUEST);
    }

    // Worker submits report back to parent cleanly without ticketPath.
    let (sub_status, _) = call(
        &state,
        &id1,
        json!({"action":"submit","status":"done","summary":"Finished task one","report":"Details"}),
    )
    .await;
    assert_eq!(sub_status, StatusCode::OK);
    let (res_status, res_body) = call(&state, "parent", json!({"action":"result","id":id1})).await;
    assert_eq!(res_status, StatusCode::OK);
    assert_eq!(res_body["reports"].as_array().unwrap().len(), 1);
    assert!(driver
        .prompts
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .contains("Sub-agent report from worker"));
    // Reviewers still require ticketPath and startingPoint even when omitted from JSON.
    assert_eq!(
        call(&state, "parent", json!({"action":"spawn","role":"spec"}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn scout_spawn_creates_fresh_sessions_and_steer_reuses_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;

    assert_eq!(
        call(&state, "parent", json!({"action":"spawn","role":"scout"}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &state,
            "parent",
            json!({"action":"spawn","role":"scout","text":"inspect","ticketPath":"ticket.md"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    let ((s1, b1), (s2, b2)) = tokio::join!(
        call(
            &state,
            "parent",
            json!({"action":"spawn","role":"scout","text":"explore module a"}),
        ),
        call(
            &state,
            "parent",
            json!({"action":"spawn","role":"scout","text":"explore module b","title":"Auth Scout"}),
        ),
    );
    assert_eq!(s1, StatusCode::OK, "{b1}");
    assert_eq!(b1["role"], "scout");
    assert_eq!(b1["tier"], "mid");
    let id1 = b1["id"].as_str().unwrap().to_string();

    assert_eq!(s2, StatusCode::OK, "{b2}");
    let id2 = b2["id"].as_str().unwrap().to_string();
    assert_ne!(id1, id2);
    assert_eq!(state.sessions.get(&id1).await.unwrap().title, "Scout (mid)");
    assert_eq!(state.sessions.get(&id2).await.unwrap().title, "Auth Scout");
    let prompted = driver.prompts.lock().unwrap().clone();
    assert!(prompted.contains(&"explore module a".to_string()));
    assert!(prompted.contains(&"explore module b".to_string()));

    let (st, sb) = call(
        &state,
        "parent",
        json!({"action":"steer","id":id1,"text":"check callers too"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{sb}");
    assert_eq!(driver.prompts.lock().unwrap()[2], "check callers too");

    state.steers.queue(&id1).set_turn_active(true).await;
    let (qt, qb) = call(
        &state,
        "parent",
        json!({"action":"steer","id":id1,"text":"queued scout note"}),
    )
    .await;
    assert_eq!(qt, StatusCode::OK, "{qb}");
    assert_eq!(driver.prompts.lock().unwrap().len(), 3);
    assert_eq!(state.steers.queue(&id1).entries().await.len(), 1);
    state.steers.queue(&id1).set_turn_active(false).await;
    assert_eq!(
        call(
            &state,
            &id1,
            json!({"action":"spawn","role":"scout","text":"nested"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    let (sub_status, _) = call(
        &state,
        &id1,
        json!({"action":"submit","status":"done","summary":"Found callers","report":"src/lib.rs:10"}),
    )
    .await;
    assert_eq!(sub_status, StatusCode::OK);
    assert!(driver
        .prompts
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .contains("Sub-agent report from scout"));
}

#[tokio::test]
async fn intelligence_tiers_configure_and_switch_workers_and_scouts_and_reject_reviewers() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _driver) = setup(dir.path()).await;
    std::fs::write(dir.path().join("ticket.md"), "ticket").unwrap();
    {
        let mut settings = state.settings.write().await;
        settings.subagent_tiers.smol =
            api::state::FallbackLadder::single("gemini-3.8-flash", Some("low".into()));
        settings.subagent_tiers.mid =
            api::state::FallbackLadder::single("gemini-3.8-flash", Some("medium".into()));
        settings.subagent_tiers.top =
            api::state::FallbackLadder::single("claude-opus-5-5", Some("high".into()));
    }

    // Reviewers reject tier on spawn and steer.
    assert_eq!(
        call(
            &state,
            "parent",
            json!({"action":"spawn","role":"spec","tier":"top","ticketPath":"ticket.md","startingPoint":"abc123"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (rs, rb) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"standards","ticketPath":"ticket.md","startingPoint":"abc123"}),
    )
    .await;
    assert_eq!(rs, StatusCode::OK, "{rb}");
    assert!(rb.get("tier").is_none());
    let reviewer_id = rb["id"].as_str().unwrap().to_string();
    assert_eq!(
        call(
            &state,
            "parent",
            json!({"action":"steer","id":reviewer_id,"tier":"smol","ticketPath":"ticket.md","startingPoint":"abc123","text":"recheck"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    // Spawn with explicit tier applies preset model, effort, and title; invalid tier fails.
    assert_eq!(
        call(
            &state,
            "parent",
            json!({"action":"spawn","role":"scout","tier":"ultra","text":"scan"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (ws, wb) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","tier":"smol","text":"quick edit"}),
    )
    .await;
    assert_eq!(ws, StatusCode::OK, "{wb}");
    assert_eq!(wb["tier"], "smol");
    let worker_id = wb["id"].as_str().unwrap().to_string();
    let worker_meta = state.sessions.get(&worker_id).await.unwrap();
    assert_eq!(worker_meta.title, "Worker (smol)");
    assert_eq!(worker_meta.model, "gemini-3.8-flash");
    assert_eq!(worker_meta.thought_level.as_deref(), Some("low"));

    let (ss, sb) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"scout","tier":"top","text":"deep trace"}),
    )
    .await;
    assert_eq!(ss, StatusCode::OK, "{sb}");
    assert_eq!(sb["tier"], "top");
    let scout_id = sb["id"].as_str().unwrap().to_string();
    let scout_meta = state.sessions.get(&scout_id).await.unwrap();
    assert_eq!(scout_meta.title, "Scout (top)");
    assert_eq!(scout_meta.model, "claude-opus-5-5");
    assert_eq!(scout_meta.thought_level.as_deref(), Some("high"));

    // Steer switches tier while idle and returns 409 Conflict when turn is active.
    let (sw_ok, sw_body) = call(
        &state,
        "parent",
        json!({"action":"steer","id":worker_id,"tier":"top","text":"harder problem"}),
    )
    .await;
    assert_eq!(sw_ok, StatusCode::OK, "{sw_body}");
    assert_eq!(sw_body["tier"], "top");
    let updated_worker = state.sessions.get(&worker_id).await.unwrap();
    assert_eq!(updated_worker.title, "Worker (top)");
    assert_eq!(updated_worker.model, "claude-opus-5-5");
    assert_eq!(updated_worker.thought_level.as_deref(), Some("high"));
    assert_eq!(
        updated_worker
            .reviewer
            .as_ref()
            .and_then(|r| r.tier.as_deref()),
        Some("top")
    );

    // Steer without tier preserves model, effort, persisted tier, and title.
    let (no_tier_ok, no_tier_body) = call(
        &state,
        "parent",
        json!({"action":"steer","id":worker_id,"text":"keep tier"}),
    )
    .await;
    assert_eq!(no_tier_ok, StatusCode::OK, "{no_tier_body}");
    assert!(no_tier_body.get("tier").is_none());
    let preserved_worker = state.sessions.get(&worker_id).await.unwrap();
    assert_eq!(preserved_worker.title, "Worker (top)");
    assert_eq!(preserved_worker.model, "claude-opus-5-5");
    assert_eq!(preserved_worker.thought_level.as_deref(), Some("high"));

    // Steer switches scout tier while idle and returns 409 Conflict when turn is active.
    let (sc_ok, sc_body) = call(
        &state,
        "parent",
        json!({"action":"steer","id":scout_id,"tier":"smol","text":"quick follow-up"}),
    )
    .await;
    assert_eq!(sc_ok, StatusCode::OK, "{sc_body}");
    assert_eq!(sc_body["tier"], "smol");
    let updated_scout = state.sessions.get(&scout_id).await.unwrap();
    assert_eq!(updated_scout.title, "Scout (smol)");
    assert_eq!(updated_scout.model, "gemini-3.8-flash");
    assert_eq!(updated_scout.thought_level.as_deref(), Some("low"));
    assert_eq!(
        updated_scout
            .reviewer
            .as_ref()
            .and_then(|r| r.tier.as_deref()),
        Some("smol")
    );

    state.steers.queue(&scout_id).set_turn_active(true).await;
    let (scout_conflict, _) = call(
        &state,
        "parent",
        json!({"action":"steer","id":scout_id,"tier":"top","text":"escalate"}),
    )
    .await;
    assert_eq!(scout_conflict, StatusCode::CONFLICT);
    state.steers.queue(&scout_id).set_turn_active(false).await;

    state.steers.queue(&worker_id).set_turn_active(true).await;
    let (conflict_status, _) = call(
        &state,
        "parent",
        json!({"action":"steer","id":worker_id,"tier":"smol","text":"downgrade"}),
    )
    .await;
    assert_eq!(conflict_status, StatusCode::CONFLICT);
    state.steers.queue(&worker_id).set_turn_active(false).await;
    state.steers.queue(&worker_id).set_turn_active(false).await;
}

struct PiListed;

#[async_trait]
impl api::models::Source for PiListed {
    fn name(&self) -> &'static str {
        "pi"
    }
    async fn fetch(&self) -> Result<Vec<api::models::ModelEntry>, api::models::CatalogueError> {
        Ok(vec![api::models::ModelEntry {
            id: "pi-sonnet".into(),
            provider: "anthropic".into(),
            label: "Pi Sonnet".into(),
            kind: "chat".into(),
            efforts: vec!["low".into(), "high".into()],
            image_input: true,
            context_window: 200_000,
            source: "pi".into(),
            stale: false,
            updated_at: 0,
        }])
    }
}

async fn setup_with_sources(
    path: &std::path::Path,
    backoffs: [std::time::Duration; 2],
) -> (AppState, Arc<Driver>) {
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
    let driver = Arc::new(Driver {
        sessions: Some(sessions.clone()),
        ..Driver::default()
    });
    let models = Arc::new(api::models::ModelCatalogue::new(
        root.model_catalogue_file(),
        vec![Arc::new(Listed), Arc::new(PiListed)],
    ));
    models.refresh().await.unwrap();
    let state = AppState::new(root, projects, sessions, Arc::new(EventBus::default()))
        .with_driver(driver.clone())
        .with_models(models)
        .with_subagent_retry_backoffs(backoffs);
    (state, driver)
}

#[tokio::test]
async fn fallback_ladder_retries_transient_twice_then_escalates_and_only_notifies_on_exhaustion() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup_with_sources(dir.path(), [std::time::Duration::ZERO; 2]).await;
    {
        let mut settings = state.settings.write().await;
        settings.subagent_tiers.mid = api::state::FallbackLadder(vec![
            api::state::ModelRung {
                model: "gemini-3.8-flash".into(),
                reasoning_effort: Some("low".into()),
            },
            api::state::ModelRung {
                model: "claude-opus-5-5".into(),
                reasoning_effort: Some("high".into()),
            },
        ]);
    }
    driver.outcomes.lock().unwrap().extend([
        Err("429 Too Many Requests".into()),
        Err("503 Service Unavailable".into()),
        Err("request timed out".into()),
        Ok(()),
    ]);

    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"fix tests"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().unwrap();
    let prompts = driver.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 4);
    assert!(prompts.iter().all(|p| p == "fix tests"));
    let meta = state.sessions.get(id).await.unwrap();
    assert_eq!(meta.model, "claude-opus-5-5");
    assert_eq!(meta.thought_level.as_deref(), Some("high"));
    assert!(state.steers.queue("parent").entries().await.is_empty());

    // Exhausting all rungs notifies parent exactly once with the final failure.
    driver.prompts.lock().unwrap().clear();
    driver
        .outcomes
        .lock()
        .unwrap()
        .extend([Err("401 Unauthorized".into()), Err("403 Forbidden".into())]);
    let (err_status, _) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"scout","text":"scan repo"}),
    )
    .await;
    assert_eq!(err_status, StatusCode::INTERNAL_SERVER_ERROR);
    // Hard errors skip same-rung retries: 1 attempt on rung 1 + 1 attempt on rung 2.
    assert_eq!(
        driver
            .prompts
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.as_str() == "scan repo")
            .count(),
        2
    );
    let entries = state.steers.queue("parent").entries().await;
    assert_eq!(entries.len(), 1);
    let report = entries[0].report.as_ref().unwrap();
    assert_eq!(report.status, "error");
    assert!(report.report.contains("403 Forbidden"));
}

#[tokio::test]
async fn fallback_ladder_cross_harness_clears_resume_token_and_uses_continuation_after_progress() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup_with_sources(dir.path(), [std::time::Duration::ZERO; 2]).await;
    {
        let mut settings = state.settings.write().await;
        settings.subagent_tiers.smol = api::state::FallbackLadder(vec![
            api::state::ModelRung {
                model: "gemini-3.8-flash".into(),
                reasoning_effort: None,
            },
            api::state::ModelRung {
                model: "pi-sonnet".into(),
                reasoning_effort: Some("high".into()),
            },
        ]);
    }
    let watcher = api::subagents::watch_errors(state.clone());
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","tier":"smol","text":"refactor module"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().unwrap().to_string();

    // Simulate an established harness resume token and partial progress before an async error.
    let mut meta = state.sessions.get(&id).await.unwrap();
    meta.resume_token = Some("agy-resume-123".into());
    state.sessions.save(&meta).await.unwrap();
    assert_eq!(meta.model, "gemini-3.8-flash");
    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            &id,
            1,
            EventKind::AssistantChunk,
            json!({"text":"Started inspecting"}),
        ));
    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            &id,
            2,
            EventKind::Error,
            json!({"message":"unknown model on agy"}),
        ));

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if state.sessions.get(&id).await.unwrap().model == "pi-sonnet"
                && driver.prompts.lock().unwrap().len() >= 2
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let migrated = state.sessions.get(&id).await.unwrap();
    assert_eq!(migrated.model, "pi-sonnet");
    assert_eq!(migrated.resume_token, None);
    assert_eq!(migrated.thought_level.as_deref(), Some("high"));
    assert_eq!(
        driver.configured.lock().unwrap().as_slice(),
        &[(
            id.clone(),
            "pi-sonnet".to_string(),
            Some("high".to_string())
        )]
    );
    let prompts = driver.prompts.lock().unwrap().clone();
    assert_eq!(prompts[0], "refactor module");
    assert!(prompts[1].contains("Continue the same task"));
    assert!(state.steers.queue("parent").entries().await.is_empty());
    watcher.abort();
}

#[tokio::test]
async fn manual_stop_never_retries_or_escalates_during_backoff() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup_with_sources(
        dir.path(),
        [
            std::time::Duration::from_secs(30),
            std::time::Duration::from_secs(30),
        ],
    )
    .await;
    {
        let mut settings = state.settings.write().await;
        settings.subagent_tiers.mid = api::state::FallbackLadder(vec![
            api::state::ModelRung {
                model: "gemini-3.8-flash".into(),
                reasoning_effort: None,
            },
            api::state::ModelRung {
                model: "claude-opus-5-5".into(),
                reasoning_effort: None,
            },
        ]);
    }
    let watcher = api::subagents::watch_errors(state.clone());
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"long work"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().unwrap().to_string();

    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            &id,
            1,
            EventKind::Error,
            json!({"message":"429 rate limit"}),
        ));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    let (stop_status, _) = call(&state, "parent", json!({"action":"stop","id":id})).await;
    assert_eq!(stop_status, StatusCode::OK);
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    assert_eq!(
        driver
            .prompts
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.as_str() == "long work")
            .count(),
        1
    );
    assert_eq!(
        state.sessions.get(&id).await.unwrap().model,
        "gemini-3.8-flash"
    );
    let entries = state.steers.queue("parent").entries().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].report.as_ref().unwrap().status, "stopped");
    watcher.abort();
}

#[tokio::test]
async fn fallback_ladder_second_steer_resets_rung_and_retry_budget() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup_with_sources(dir.path(), [std::time::Duration::ZERO; 2]).await;
    {
        let mut settings = state.settings.write().await;
        settings.subagent_tiers.mid = api::state::FallbackLadder(vec![
            api::state::ModelRung {
                model: "gemini-3.8-flash".into(),
                reasoning_effort: Some("low".into()),
            },
            api::state::ModelRung {
                model: "claude-opus-5-5".into(),
                reasoning_effort: Some("high".into()),
            },
        ]);
    }
    driver
        .outcomes
        .lock()
        .unwrap()
        .extend([Err("401 Unauthorized".into()), Ok(())]);
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"task one"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(
        state.sessions.get(&id).await.unwrap().model,
        "claude-opus-5-5"
    );

    // Explicit steer with tier resets back to rung 0 and gives a fresh 2-retry transient budget.
    driver.outcomes.lock().unwrap().extend([
        Err("429 Too Many Requests".into()),
        Err("503 Service Unavailable".into()),
        Ok(()),
    ]);
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"steer","id":id,"tier":"mid","text":"task two"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        state.sessions.get(&id).await.unwrap().model,
        "gemini-3.8-flash"
    );
    assert_eq!(
        driver
            .prompts
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.as_str() == "task two")
            .count(),
        3
    );

    // Subsequent steer without tier still starts a fresh run with a full 2-retry budget.
    driver.outcomes.lock().unwrap().extend([
        Err("429 Too Many Requests".into()),
        Err("503 Service Unavailable".into()),
        Ok(()),
    ]);
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"steer","id":id,"text":"task three"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        driver
            .prompts
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.as_str() == "task three")
            .count(),
        3
    );
}

#[tokio::test]
async fn fallback_ladder_exhausts_three_rungs_and_clears_effort_when_none() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup_with_sources(dir.path(), [std::time::Duration::ZERO; 2]).await;
    {
        let mut settings = state.settings.write().await;
        settings.subagent_tiers.mid = api::state::FallbackLadder(vec![
            api::state::ModelRung {
                model: "gemini-3.8-flash".into(),
                reasoning_effort: Some("low".into()),
            },
            api::state::ModelRung {
                model: "claude-opus-5-5".into(),
                reasoning_effort: None,
            },
            api::state::ModelRung {
                model: "pi-sonnet".into(),
                reasoning_effort: Some("high".into()),
            },
        ]);
    }
    driver.outcomes.lock().unwrap().extend([
        Err("401 Unauthorized".into()),
        Err("503 Service Unavailable".into()),
        Err("503 Service Unavailable".into()),
        Err("503 Service Unavailable".into()),
        Err("403 Forbidden".into()),
    ]);

    let (status, _) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"three rungs"}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let configured: Vec<_> = driver
        .configured
        .lock()
        .unwrap()
        .iter()
        .map(|(_, model, effort)| (model.clone(), effort.clone()))
        .collect();
    assert_eq!(
        configured,
        vec![
            ("claude-opus-5-5".to_string(), None),
            ("pi-sonnet".to_string(), Some("high".to_string())),
        ]
    );
    let entries = state.steers.queue("parent").entries().await;
    assert_eq!(entries.len(), 1);
    let report = entries[0].report.as_ref().unwrap();
    assert_eq!(report.status, "error");
    assert!(report.report.contains("403 Forbidden"));
}

#[tokio::test]
async fn watch_errors_async_recovery_resets_transient_budget_and_terminal_ignores_duplicate_errors()
{
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup_with_sources(dir.path(), [std::time::Duration::ZERO; 2]).await;
    {
        let mut settings = state.settings.write().await;
        settings.subagent_tiers.mid = api::state::FallbackLadder(vec![
            api::state::ModelRung {
                model: "gemini-3.8-flash".into(),
                reasoning_effort: None,
            },
            api::state::ModelRung {
                model: "claude-opus-5-5".into(),
                reasoning_effort: None,
            },
        ]);
    }
    let watcher = api::subagents::watch_errors(state.clone());
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"async resilience"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    // Keep the parent busy so the exhaustion report stays queued instead of being
    // delivered as an extra driver prompt by `wake_parent` -> `deliver_idle`.
    state.steers.queue("parent").set_turn_active(true).await;

    // Incident 1: transient async error recovers after 1 retry.
    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            &id,
            1,
            EventKind::AssistantChunk,
            json!({"text":"working"}),
        ));
    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            &id,
            2,
            EventKind::Error,
            json!({"message":"429 Too Many Requests"}),
        ));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if driver.prompts.lock().unwrap().len() >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    // Incident 2 on same run: transient budget reset on recovery so it still gets 2 retries before escalating, then exhausts.
    driver.outcomes.lock().unwrap().extend([
        Err("503 Service Unavailable".into()),
        Err("503 Service Unavailable".into()),
        Err("403 Forbidden".into()),
    ]);
    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            &id,
            3,
            EventKind::Error,
            json!({"message":"429 rate limit"}),
        ));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if !state.steers.queue("parent").entries().await.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // 1 initial + 1 incident-1 retry + 2 incident-2 retries + 1 escalation = 5 prompts.
    let prompts = driver.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 5, "{prompts:?}");
    assert_eq!(state.steers.queue("parent").entries().await.len(), 1);

    // Stray Error event after exhaustion must not notify parent a second time.
    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            &id,
            4,
            EventKind::Error,
            json!({"message":"late trailing error"}),
        ));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(state.steers.queue("parent").entries().await.len(), 1);

    // Deleting the child session removes any run entry and handles delete cleanly.
    let deleted = api::routes::delete_session(
        axum::extract::State(state.clone()),
        axum::extract::Path(id.clone()),
    )
    .await;
    assert!(deleted.is_ok());
    watcher.abort();
}

fn emit(state: &AppState, id: &str, seq: u64, kind: EventKind, data: Value) {
    state
        .event_bus
        .publish(emberloop_core::events::SessionEvent::new(
            id, seq, kind, data,
        ));
}

async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_text_only_turn_is_nudged_and_an_accepted_submit_ends_the_nudges() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    let watcher = api::subagents::watch_submits(state.clone());
    state.steers.queue("parent").set_turn_active(true).await;
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"fix the bug"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(driver.prompts.lock().unwrap().as_slice(), ["fix the bug"]);

    // A rejected submit is not an accepted report, so this turn still gets nudged.
    let rejected = call(&state, &id, json!({"action":"submit","status":"pass"})).await;
    assert_eq!(rejected.0, StatusCode::BAD_REQUEST);
    emit(
        &state,
        &id,
        1,
        EventKind::AssistantChunk,
        json!({"text":"I fixed it."}),
    );
    emit(&state, &id, 2, EventKind::Status, json!({"state":"idle"}));
    until(|| driver.prompts.lock().unwrap().len() == 2).await;
    assert!(driver.prompts.lock().unwrap()[1].contains("Call submit"));

    let (status, _) = call(
        &state,
        &id,
        json!({"action":"submit","status":"done","report":"Fixed it","summary":"fixed"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    emit(&state, &id, 3, EventKind::Status, json!({"state":"idle"}));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        driver.prompts.lock().unwrap().len(),
        2,
        "no nudge after a report"
    );
    let entries = state.steers.queue("parent").entries().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].report.as_ref().unwrap().status, "done");
    watcher.abort();
}

#[tokio::test]
async fn a_subagent_that_never_submits_gets_three_nudges_then_a_blocked_report() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    let watcher = api::subagents::watch_submits(state.clone());
    state.steers.queue("parent").set_turn_active(true).await;
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"keep trying"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().unwrap().to_string();

    for attempt in 1..=3 {
        emit(
            &state,
            &id,
            attempt,
            EventKind::AssistantChunk,
            json!({"text":format!("attempt {attempt}")}),
        );
        emit(
            &state,
            &id,
            attempt,
            EventKind::Status,
            json!({"state":"idle"}),
        );
        until(|| driver.prompts.lock().unwrap().len() == attempt as usize + 1).await;
    }
    assert_eq!(driver.prompts.lock().unwrap().len(), 4);
    emit(
        &state,
        &id,
        4,
        EventKind::AssistantChunk,
        json!({"text":"last words"}),
    );
    emit(&state, &id, 4, EventKind::Status, json!({"state":"idle"}));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state.steers.queue("parent").entries().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let entries = state.steers.queue("parent").entries().await;
    assert_eq!(entries.len(), 1);
    let report = entries[0].report.as_ref().unwrap();
    assert_eq!(report.status, "blocked");
    assert!(report.report.contains("last words"), "{}", report.report);
    assert_eq!(driver.prompts.lock().unwrap().len(), 4, "bound holds");
    watcher.abort();
}

#[tokio::test]
async fn a_first_time_submit_and_plain_sessions_are_not_nudged() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    let watcher = api::subagents::watch_submits(state.clone());
    state.steers.queue("parent").set_turn_active(true).await;
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"fix the bug"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &state,
        &id,
        json!({"action":"submit","status":"done","report":"Done","summary":"done"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    emit(&state, &id, 1, EventKind::Status, json!({"state":"idle"}));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        driver.prompts.lock().unwrap().len(),
        1,
        "no nudge after a report"
    );
    assert_eq!(state.steers.queue("parent").entries().await.len(), 1);

    // A Session that is not a subagent is never nudged or reported on.
    emit(
        &state,
        "parent",
        1,
        EventKind::AssistantChunk,
        json!({"text":"done"}),
    );
    emit(
        &state,
        "parent",
        2,
        EventKind::Status,
        json!({"state":"idle"}),
    );
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(driver.prompts.lock().unwrap().len(), 1);
    assert_eq!(state.steers.queue("parent").entries().await.len(), 1);
    watcher.abort();
}

fn write_persona(dir: &std::path::Path, name: &str) {
    let agents = dir.join(".agents/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join(format!("{name}.md")),
        format!("---\nname: {name}\ndescription: {name} persona\n---\nYou are {name}.\n"),
    )
    .unwrap();
}

async fn post(state: &AppState, uri: &str, body: Value) -> StatusCode {
    build_router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn goldfish(state: &AppState, persona: &str, extra: Value) -> (StatusCode, Value) {
    let mut body = json!({"action":"spawn","role":"goldfish","persona":persona,"text":"review"});
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    call(state, "parent", body).await
}

#[tokio::test]
async fn goldfish_spawn_validates_persona_and_tier() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    write_persona(dir.path(), "elephant");
    write_persona(dir.path(), "critic");

    let (status, body) = goldfish(&state, "nobody", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let message = body.to_string();
    assert!(message.contains("nobody"), "{message}");
    assert!(
        message.contains("elephant") && message.contains("critic"),
        "{message}"
    );
    let missing = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"goldfish","text":"review"}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::BAD_REQUEST);
    let on_worker = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"x","persona":"elephant"}),
    )
    .await;
    assert_eq!(on_worker.0, StatusCode::BAD_REQUEST);
    let (bad_tier, _) = goldfish(&state, "elephant", json!({"tier":"huge"})).await;
    assert_eq!(bad_tier, StatusCode::BAD_REQUEST);
    assert!(driver.prompts.lock().unwrap().is_empty());

    let (status, body) = goldfish(&state, "elephant", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["role"], "goldfish");
    assert_eq!(body["tier"], "mid");
    let id = body["id"].as_str().unwrap();
    let meta = state.sessions.get(id).await.unwrap();
    assert_eq!(meta.title, "Goldfish (elephant)");
    let assignment = meta.reviewer.unwrap();
    assert_eq!(assignment.role, "goldfish");
    assert_eq!(assignment.persona.as_deref(), Some("elephant"));
    for tier in ["smol", "mid", "top"] {
        let (status, body) = goldfish(&state, "critic", json!({"tier":tier,"title":"T"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["tier"], tier);
    }
}

#[tokio::test]
async fn a_goldfish_walks_its_own_tier_ladder_and_inherits_an_empty_one() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _driver) = setup(dir.path()).await;
    write_persona(dir.path(), "elephant");
    {
        let mut settings = state.settings.write().await;
        settings.subagent_tiers.smol =
            api::state::FallbackLadder::single("gemini-3.8-flash", Some("low".into()));
        settings.goldfish_tiers.top =
            api::state::FallbackLadder::single("claude-opus-5-5", Some("high".into()));
    }

    let (status, body) = goldfish(&state, "elephant", json!({"tier":"top"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let meta = state
        .sessions
        .get(body["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(meta.model, "claude-opus-5-5");
    assert_eq!(meta.thought_level.as_deref(), Some("high"));

    // The goldfish smol ladder is empty, so the worker/scout smol ladder is walked.
    let (status, body) = goldfish(&state, "elephant", json!({"tier":"smol"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let meta = state
        .sessions
        .get(body["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(meta.model, "gemini-3.8-flash");
    assert_eq!(meta.thought_level.as_deref(), Some("low"));
}

#[tokio::test]
async fn goldfish_gets_a_fresh_session_per_call_and_cannot_nest() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _driver) = setup(dir.path()).await;
    write_persona(dir.path(), "elephant");
    let (_, a) = goldfish(&state, "elephant", json!({})).await;
    let (_, b) = goldfish(&state, "elephant", json!({})).await;
    assert_ne!(a["id"], b["id"]);
    let id = a["id"].as_str().unwrap();
    let nested = call(
        &state,
        id,
        json!({"action":"spawn","role":"goldfish","persona":"elephant","text":"x"}),
    )
    .await;
    assert_eq!(nested.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn goldfish_rejects_follow_up_through_all_three_entry_points() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    write_persona(dir.path(), "elephant");
    let (_, body) = goldfish(&state, "elephant", json!({})).await;
    let id = body["id"].as_str().unwrap().to_string();
    let before = driver.prompts.lock().unwrap().len();

    let steer = call(
        &state,
        "parent",
        json!({"action":"steer","id":id,"text":"more"}),
    )
    .await;
    assert_eq!(steer.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        post(
            &state,
            &format!("/api/sessions/{id}/prompt"),
            json!({"text":"more"})
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(
            &state,
            &format!("/api/sessions/{id}/steer"),
            json!({"text":"more"})
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(driver.prompts.lock().unwrap().len(), before);
}

#[tokio::test]
async fn goldfish_recovery_keeps_the_persona_across_the_ladder() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup_with_sources(dir.path(), [std::time::Duration::ZERO; 2]).await;
    write_persona(dir.path(), "elephant");
    {
        let mut settings = state.settings.write().await;
        settings.goldfish_tiers.mid = api::state::FallbackLadder(vec![
            api::state::ModelRung {
                model: "gemini-3.8-flash".into(),
                reasoning_effort: None,
            },
            api::state::ModelRung {
                model: "claude-opus-5-5".into(),
                reasoning_effort: None,
            },
        ]);
    }
    driver
        .outcomes
        .lock()
        .unwrap()
        .extend([Err("401 Unauthorized".into()), Ok(())]);
    let (status, body) = goldfish(&state, "elephant", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let meta = state
        .sessions
        .get(body["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(meta.model, "claude-opus-5-5");
    assert_eq!(meta.reviewer.unwrap().persona.as_deref(), Some("elephant"));
    assert_eq!(driver.prompts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn goldfish_recovery_falls_back_to_the_subagent_ladder_when_its_own_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup_with_sources(dir.path(), [std::time::Duration::ZERO; 2]).await;
    write_persona(dir.path(), "elephant");
    {
        let mut settings = state.settings.write().await;
        settings.subagent_tiers.mid = api::state::FallbackLadder(vec![
            api::state::ModelRung {
                model: "gemini-3.8-flash".into(),
                reasoning_effort: None,
            },
            api::state::ModelRung {
                model: "claude-opus-5-5".into(),
                reasoning_effort: None,
            },
        ]);
        assert!(settings.goldfish_tiers.mid.is_empty());
    }
    driver
        .outcomes
        .lock()
        .unwrap()
        .extend([Err("401 Unauthorized".into()), Ok(())]);
    let (status, body) = goldfish(&state, "elephant", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let meta = state
        .sessions
        .get(body["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(meta.model, "claude-opus-5-5");
    assert_eq!(meta.reviewer.unwrap().persona.as_deref(), Some("elephant"));
    assert_eq!(driver.prompts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn goldfish_is_stopped_after_its_report_and_never_resurrected() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    write_persona(dir.path(), "elephant");
    let submits = api::subagents::watch_submits(state.clone());
    let errors = api::subagents::watch_errors(state.clone());
    state.steers.queue("parent").set_turn_active(true).await;
    let (_, body) = goldfish(&state, "elephant", json!({})).await;
    let id = body["id"].as_str().unwrap().to_string();

    let (status, _) = call(
        &state,
        &id,
        json!({"action":"submit","status":"done","summary":"s","report":"r"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        driver.cancelled.lock().unwrap().is_empty(),
        "stop waits for idle"
    );
    emit(&state, &id, 1, EventKind::Status, json!({"state":"idle"}));
    until(|| driver.cancelled.lock().unwrap().contains(&id)).await;

    emit(
        &state,
        &id,
        2,
        EventKind::Error,
        json!({"message":"429 rate limit"}),
    );
    emit(&state, &id, 3, EventKind::Status, json!({"state":"idle"}));
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        driver.prompts.lock().unwrap().len(),
        1,
        "no nudge, no retry"
    );
    let entries = state.steers.queue("parent").entries().await;
    assert_eq!(entries.len(), 1, "only the submit report");
    assert_eq!(entries[0].report.as_ref().unwrap().status, "done");
    assert!(state.sessions.get(&id).await.is_some(), "transcript stays");
    submits.abort();
    errors.abort();
}

#[tokio::test]
async fn a_goldfish_that_never_submits_is_blocked_then_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    write_persona(dir.path(), "elephant");
    let submits = api::subagents::watch_submits(state.clone());
    state.steers.queue("parent").set_turn_active(true).await;
    let (_, body) = goldfish(&state, "elephant", json!({})).await;
    let id = body["id"].as_str().unwrap().to_string();

    for seq in 1..5 {
        emit(&state, &id, seq, EventKind::Status, json!({"state":"idle"}));
    }
    until(|| driver.cancelled.lock().unwrap().contains(&id)).await;
    let entries = state.steers.queue("parent").entries().await;
    assert_eq!(entries.len(), 1, "only the blocked report");
    assert_eq!(entries[0].report.as_ref().unwrap().status, "blocked");
    assert_eq!(
        driver.prompts.lock().unwrap().len(),
        4,
        "spawn and three nudges"
    );
    submits.abort();
}

#[tokio::test]
async fn goldfish_spawn_reports_a_missing_parent_project() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    write_persona(dir.path(), "elephant");
    state.projects.delete("p").await.unwrap();

    let (status, body) = goldfish(&state, "elephant", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.to_string().contains("project was not found"), "{body}");
    assert!(driver.prompts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn batch_names_are_validated_and_batches_seal_with_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = setup(dir.path()).await;
    for batch in [json!("   "), json!("x".repeat(65)), json!("bad\nname")] {
        let (status, body) = call(
            &state,
            "parent",
            json!({"action":"spawn","role":"worker","text":"work","batch":batch}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    state.steers.queue("parent").set_turn_active(true).await;
    let first = spawn_batch(&state, "worker", "first", "fleet").await;
    assert_eq!(first.0, StatusCode::OK);
    state.steers.queue("parent").set_turn_active(false).await;
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"worker","text":"late","batch":"fleet"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("sealed"), "{body}");
}

#[tokio::test]
async fn duplicate_pending_reviewer_reuse_in_a_batch_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = setup(dir.path()).await;
    state.steers.queue("parent").set_turn_active(true).await;
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"spec","ticketPath":"ticket.md","startingPoint":"abc123","batch":"review"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &state,
        "parent",
        json!({"action":"spawn","role":"spec","ticketPath":"next.md","startingPoint":"abc123","batch":"review"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("already pending"), "{body}");
}

#[tokio::test]
async fn batched_reports_stay_held_until_the_sealed_batch_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    state.steers.queue("parent").set_turn_active(true).await;
    let (_, body) = spawn_batch(&state, "worker", "one", "fleet").await;
    let one = body["id"].as_str().unwrap().to_string();
    let (_, body) = spawn_batch(&state, "worker", "two", "fleet").await;
    let two = body["id"].as_str().unwrap().to_string();

    let (status, reply) = call(
        &state,
        &one,
        json!({"action":"submit","status":"done","report":"first report"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["heldInBatch"], "fleet", "{reply}");
    let queued = state.steers.queue("parent");
    assert_eq!(queued.pending_len().await, 0);
    assert_eq!(
        queued.entries().await[0].state,
        emberloop_core::steer::SteerState::Held
    );
    assert!(state
        .get_event_log("parent")
        .await
        .unwrap()
        .kind_tail(&EventKind::SteerQueued, 10)
        .await
        .unwrap()
        .is_empty());
    let (status, results) = call(&state, "parent", json!({"action":"result","id":one})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(results["reports"].as_array().unwrap().len(), 1);
    let user = queued
        .enqueue(emberloop_core::steer::SteerKind::Steer, "user prompt")
        .await;
    let drained = queued.drain_pending().await;
    assert_eq!(drained.len(), 1, "held reports never ride a user prompt");
    assert_eq!(drained[0].id, user.id);
    queued.mark_delivered(&[user.id], "test").await;

    queued.set_turn_active(false).await;
    driver.prompts.lock().unwrap().clear();
    let (status, reply) = call(
        &state,
        &two,
        json!({"action":"submit","status":"done","report":"second report"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert!(reply.get("heldInBatch").is_none(), "{reply}");
    let events = state
        .get_event_log("parent")
        .await
        .unwrap()
        .kind_tail(&EventKind::SteerQueued, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 2, "one event per released report");
    assert!(!queued.has_open_batch().await);
    let prompts = driver.prompts.lock().unwrap();
    assert_eq!(
        prompts.len(),
        1,
        "one wake for the whole batch: {prompts:?}"
    );
    assert!(
        prompts[0].contains("Batch \"fleet\" completed (2/2):"),
        "{}",
        prompts[0]
    );
    assert!(prompts[0].contains("first report"));
    assert!(prompts[0].contains("second report"));
}

async fn spawn_batch(state: &AppState, role: &str, text: &str, batch: &str) -> (StatusCode, Value) {
    call(
        state,
        "parent",
        json!({"action":"spawn","role":role,"text":text,"batch":batch}),
    )
    .await
}

#[tokio::test]
async fn a_failed_worker_dispatch_delivers_an_error_report_inside_the_batch() {
    let dir = tempfile::tempdir().unwrap();
    let (state, driver) = setup(dir.path()).await;
    state.steers.queue("parent").set_turn_active(true).await;
    let (_, body) = spawn_batch(&state, "worker", "one", "fleet").await;
    let one = body["id"].as_str().unwrap().to_string();
    let (_, reply) = call(
        &state,
        &one,
        json!({"action":"submit","status":"done","report":"first"}),
    )
    .await;
    assert_eq!(reply["heldInBatch"], "fleet", "{reply}");
    for _ in 0..10 {
        driver
            .outcomes
            .lock()
            .unwrap()
            .push_back(Err("dispatch failed".into()));
    }
    let (status, body) = spawn_batch(&state, "worker", "two", "fleet").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");

    let queued = state.steers.queue("parent");
    let entries = queued.entries().await;
    assert_eq!(entries.len(), 2);
    assert!(entries
        .iter()
        .all(|entry| entry.state == emberloop_core::steer::SteerState::Held));
    assert!(entries
        .iter()
        .any(|entry| entry.report.as_ref().is_some_and(|r| r.status == "error")));
    let released = queued.set_turn_active(false).await;
    assert_eq!(released.len(), 3, "notice plus two reports");
    assert_eq!(released[0].text, "Batch \"fleet\" completed (2/2):");
    assert_eq!(queued.pending_len().await, 3);
    assert!(!queued.has_open_batch().await);
}

#[tokio::test]
async fn recovery_reconciles_open_batches_and_publishes_released_reports() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = setup(dir.path()).await;
    let queue = state.steers.queue("parent");
    queue.set_turn_active(true).await;
    for child in ["child1", "child2"] {
        queue
            .register_batch_member(
                "fleet",
                child,
                emberloop_core::steer::DEFAULT_BATCH_DEADLINE_MS,
            )
            .await
            .unwrap();
    }
    queue
        .enqueue_report(emberloop_core::steer::SubagentReport {
            session_id: "child1".into(),
            role: "worker".into(),
            ticket_path: String::new(),
            status: "done".into(),
            summary: "first".into(),
            report: "first report".into(),
        })
        .await
        .unwrap();
    queue.complete_batch_member("child1").await;
    let root = state.data_root.clone();
    drop(state);

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

    let prompts = driver.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].contains("Batch \"fleet\" completed (2/2):"));
    assert!(prompts[0].contains("first report"));
    assert!(prompts[0].contains("errored"));
    assert_eq!(
        restored
            .get_event_log("parent")
            .await
            .unwrap()
            .kind_tail(&EventKind::SteerQueued, 10)
            .await
            .unwrap()
            .len(),
        2
    );
    let queue = restored.steers.queue("parent");
    assert!(!queue.has_open_batch().await);
    assert_eq!(
        queue.pending_len().await,
        0,
        "the wake delivered everything"
    );
    let reports: Vec<_> = queue
        .entries()
        .await
        .into_iter()
        .filter_map(|entry| entry.report)
        .collect();
    assert_eq!(reports.len(), 2);
    assert!(reports.iter().any(|report| report.status == "done"));
    assert!(reports.iter().any(|report| report.status == "errored"));
}

#[tokio::test]
async fn recovery_keeps_held_reports_out_of_the_feed() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = setup(dir.path()).await;
    let queue = state.steers.queue("parent");
    queue.set_turn_active(true).await;
    for child in ["child1", "child2"] {
        queue
            .register_batch_member(
                "fleet",
                child,
                emberloop_core::steer::DEFAULT_BATCH_DEADLINE_MS,
            )
            .await
            .unwrap();
    }
    queue
        .enqueue_report(emberloop_core::steer::SubagentReport {
            session_id: "child1".into(),
            role: "worker".into(),
            ticket_path: String::new(),
            status: "done".into(),
            summary: "first".into(),
            report: "first report".into(),
        })
        .await
        .unwrap();
    queue.complete_batch_member("child1").await;

    api::subagents::recover(&state).await.unwrap();

    assert_eq!(
        state
            .get_event_log("parent")
            .await
            .unwrap()
            .kind_tail(&EventKind::SteerQueued, 10)
            .await
            .unwrap()
            .len(),
        0
    );
    assert!(queue.has_open_batch().await);
    assert!(queue.entries().await.iter().any(|entry| {
        entry.state == emberloop_core::steer::SteerState::Held
            && entry
                .report
                .as_ref()
                .is_some_and(|report| report.session_id == "child1")
    }));
}
