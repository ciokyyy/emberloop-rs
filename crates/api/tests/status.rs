#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::sync::Arc;

use api::{build_router, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use emberloop_core::events::{EventBus, EventKind, SessionEvent};
use serde_json::json;
use store::{
    ArchivalConfig, ArchivalSweeper, DataRoot, Project, ProjectsStore, SessionMeta, SessionState,
    SessionsStore,
};
use tempfile::tempdir;
use tower::ServiceExt;

async fn state(dir: &tempfile::TempDir) -> AppState {
    let data_root = DataRoot::new(dir.path());
    let projects = ProjectsStore::load(data_root.projects_file())
        .await
        .unwrap();
    let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
    AppState::new(data_root, projects, sessions, Arc::new(EventBus::default()))
}

async fn get_json(state: &AppState, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = build_router(state.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

#[tokio::test]
async fn system_status_returns_host_memory_disk_and_runtime_rows() {
    let dir = tempdir().unwrap();
    let state = state(&dir).await;
    state
        .projects
        .insert(Project {
            id: "p1".into(),
            name: "demo".into(),
            root: dir.path().to_path_buf(),
        })
        .await
        .unwrap();
    state
        .sessions
        .insert(SessionMeta {
            id: "s1".into(),
            project_id: "p1".into(),
            title: "sys".into(),
            model: "claude/sonnet".into(),
            ..SessionMeta::default()
        })
        .await
        .unwrap();

    let log = state.get_event_log("s1").await.unwrap();
    log.append(SessionEvent::new(
        "s1",
        0,
        EventKind::Status,
        json!({"state": "working"}),
    ))
    .await
    .unwrap();

    let (status, body) = get_json(&state, "/api/status/system?refresh=true").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["memory"]["totalBytes"].as_u64().unwrap() > 0);
    assert!(body["disk"]["totalBytes"].as_u64().unwrap() > 0);
    assert_eq!(body["runtime"]["activeSessions"], 1);
    assert_eq!(body["runtime"]["workingSessions"], 1);
    assert_eq!(body["storage"]["segments"].as_array().unwrap().len(), 4);
    assert!(body["processes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "sessions" && p["state"] == "Working"));
}

#[tokio::test]
async fn token_status_aggregates_active_archived_latency_and_tool_sinks() {
    let dir = tempdir().unwrap();
    let state = state(&dir).await;
    state
        .projects
        .insert(Project {
            id: "p-ember".into(),
            name: "ember".into(),
            root: dir.path().to_path_buf(),
        })
        .await
        .unwrap();

    // 2026-11-10T12:00:00Z
    let nov_ms: u64 = 1_794_312_000_000;
    // 2026-10-10T12:00:00Z
    let oct_ms: u64 = 1_791_633_600_000;

    state
        .sessions
        .insert(SessionMeta {
            id: "s-active".into(),
            project_id: "p-ember".into(),
            title: "act".into(),
            model: "claude/opus".into(),
            created_at: nov_ms - 5_000,
            updated_at: nov_ms + 5_000,
            last_used_at: Some(nov_ms + 5_000),
            ..SessionMeta::default()
        })
        .await
        .unwrap();

    let ev_path = state.data_root.session_events_dir("s-active");
    std::fs::create_dir_all(&ev_path).unwrap();
    let mut t1 = SessionEvent::new(
        "s-active",
        1,
        EventKind::ToolStarted,
        json!({"id": "c1", "name": "mcp__el__execute", "input": {}}),
    );
    t1.at = nov_ms;
    // Streaming partial-arg chunks arrive with empty tool name and must not erase "execute".
    let mut t1_chunk = SessionEvent::new(
        "s-active",
        2,
        EventKind::ToolStarted,
        json!({"id": "c1", "name": "", "input": {"code": "print(1)"}}),
    );
    t1_chunk.at = nov_ms + 50;
    let mut t1_el = SessionEvent::new(
        "s-active",
        3,
        EventKind::ToolStarted,
        json!({"id": "c2", "name": "mcp__el_read", "input": {"path": "a.rs"}}),
    );
    t1_el.at = nov_ms + 100;
    let mut t2 = SessionEvent::new(
        "s-active",
        4,
        EventKind::ToolFinished,
        json!({"id": "c1", "output": "x".repeat(4000)}),
    );
    t2.at = nov_ms + 200;
    let mut t2_el = SessionEvent::new(
        "s-active",
        5,
        EventKind::ToolFinished,
        json!({"id": "c2", "output": "y".repeat(800)}),
    );
    t2_el.at = nov_ms + 220;
    let mut u1 = SessionEvent::new(
        "s-active",
        6,
        EventKind::Usage,
        json!({
            "inputTokens": 10_000,
            "outputTokens": 500,
            "cachedTokens": 8_000,
            "cacheCreationTokens": 1_000,
            "durationMs": 2_400,
            "breakdown": {
                "system": 2_000,
                "toolDefinitions": 1_000,
                "toolCalls": 500,
                "toolResults": 3_500,
                "compression": 0,
                "conversation": 3_000,
                "images": 0
            }
        }),
    );
    u1.at = nov_ms + 2_600;
    let mut u_oct = SessionEvent::new(
        "s-active",
        4,
        EventKind::Usage,
        json!({"inputTokens": 99_999, "outputTokens": 1, "cachedTokens": 0}),
    );
    u_oct.at = oct_ms;

    let jsonl = [t1, t1_chunk, t1_el, t2, t2_el, u1, u_oct]
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(ev_path.join("0001.jsonl"), format!("{jsonl}\n")).unwrap();

    // Archived session in same month with inferred latency
    state
        .sessions
        .insert(SessionMeta {
            id: "s-arch".into(),
            project_id: "p-ember".into(),
            title: "arch".into(),
            model: "gemini-3-flash".into(),
            created_at: nov_ms,
            updated_at: nov_ms + 2_000,
            last_used_at: Some(nov_ms + 2_000),
            state: SessionState::Active,
            ..SessionMeta::default()
        })
        .await
        .unwrap();
    let arch_ev_dir = state.data_root.session_events_dir("s-arch");
    std::fs::create_dir_all(&arch_ev_dir).unwrap();
    let mut p = SessionEvent::new("s-arch", 1, EventKind::Prompt, json!({"text": "go"}));
    p.at = nov_ms;
    let mut u2 = SessionEvent::new(
        "s-arch",
        2,
        EventKind::Usage,
        json!({"inputTokens": 4_000, "outputTokens": 200, "cachedTokens": 2_000}),
    );
    u2.at = nov_ms + 1_200;
    let arch_jsonl = format!(
        "{}\n{}\n",
        serde_json::to_string(&p).unwrap(),
        serde_json::to_string(&u2).unwrap()
    );
    std::fs::write(arch_ev_dir.join("0001.jsonl"), arch_jsonl).unwrap();

    // Archive s-arch using ArchivalSweeper with 0-day threshold while keeping s-active untouched via newer timestamp.
    let sweeper = ArchivalSweeper::new(
        state.data_root.clone(),
        state.sessions.clone(),
        ArchivalConfig {
            archive_after_days: 1,
            restore_keep_hours: 1,
            archive_sweep_hours: 24,
        },
    );
    state.sessions.touch("s-arch", nov_ms).await.unwrap();
    let report = sweeper.sweep_at(nov_ms + 2 * 86_400_000).await.unwrap();
    assert!(report.archived.contains(&"s-arch".to_string()));
    assert!(!arch_ev_dir.exists());

    let (status, body) = get_json(&state, "/api/status/tokens?month=2026-11").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["month"], "2026-11");
    assert_eq!(body["summary"]["totalCalls"], 2);
    assert_eq!(body["summary"]["inputTokens"], 14_000);
    assert_eq!(body["summary"]["outputTokens"], 700);
    assert_eq!(body["summary"]["cachedTokens"], 10_000);
    assert_eq!(body["summary"]["avgLatencyMs"], 1_800);
    assert_eq!(body["rows"].as_array().unwrap().len(), 2);
    let sinks = body["sinks"].as_array().unwrap();
    let exec_tok = sinks
        .iter()
        .find(|s| s["name"] == "execute")
        .and_then(|s| s["tokens"].as_u64())
        .unwrap_or(0);
    let read_tok = sinks
        .iter()
        .find(|s| s["name"] == "read")
        .and_then(|s| s["tokens"].as_u64())
        .unwrap_or(0);
    assert!(exec_tok > 3_000 && read_tok > 500);
    assert!(exec_tok + read_tok >= 3_998);
    assert!(!sinks.iter().any(|s| s["name"] == ""));
    let (bad_status, _) = get_json(&state, "/api/status/tokens?month=bad").await;
    assert_eq!(bad_status, StatusCode::BAD_REQUEST);
}
