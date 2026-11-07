#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::sync::Arc;

use api::{build_router, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use emberloop_core::events::{EventBus, EventKind, SessionEvent};
use store::{DataRoot, ProjectsStore, SessionsStore};
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

async fn get_usage(state: &AppState, query: &str) -> (StatusCode, serde_json::Value) {
    let response = build_router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/sessions/s1/usage{query}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn record(state: &AppState, kind: EventKind, data: serde_json::Value) {
    let log = state.get_event_log("s1").await.unwrap();
    log.append(SessionEvent::new("s1", 0, kind, data))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_empty_session_has_no_usage_history() {
    let dir = tempdir().unwrap();
    let state = state(&dir).await;
    let (status, body) = get_usage(&state, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test]
async fn usage_history_lists_each_request_oldest_first_with_its_hit_percentage() {
    let dir = tempdir().unwrap();
    let state = state(&dir).await;
    // The first request reads 80000 cached and writes 5000; 2000 of the 87000 prompt is fresh.
    record(
        &state,
        EventKind::Usage,
        serde_json::json!({"inputTokens": 87000, "cachedTokens": 80000, "cacheCreationTokens": 5000, "outputTokens": 9}),
    )
    .await;
    record(
        &state,
        EventKind::Status,
        serde_json::json!({"state": "working"}),
    )
    .await;
    record(
        &state,
        EventKind::Usage,
        serde_json::json!({"inputTokens": 2000, "cachedTokens": 0, "cacheCreationTokens": 0, "outputTokens": 3}),
    )
    .await;

    let (status, body) = get_usage(&state, "").await;
    assert_eq!(status, StatusCode::OK);
    let records = body.as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records[0]["seq"].as_u64() < records[1]["seq"].as_u64());
    assert_eq!(records[0]["inputTokens"], 87000);
    assert_eq!(records[0]["freshInputTokens"], 2000);
    assert_eq!(records[0]["cachedTokens"], 80000);
    assert_eq!(records[0]["cacheCreationTokens"], 5000);
    assert_eq!(records[0]["outputTokens"], 9);
    let first = records[0]["hitPercent"].as_f64().unwrap();
    assert!((first - 80000.0 / 87000.0 * 100.0).abs() < 1e-9, "{first}");
    assert_eq!(records[1]["inputTokens"], 2000);
    assert_eq!(records[1]["freshInputTokens"], 2000);
    assert_eq!(records[1]["hitPercent"].as_f64(), Some(0.0));
}

#[tokio::test]
async fn a_request_with_no_prompt_has_no_hit_percentage() {
    let dir = tempdir().unwrap();
    let state = state(&dir).await;
    record(
        &state,
        EventKind::Usage,
        serde_json::json!({"inputTokens": 0, "cachedTokens": 0, "outputTokens": 4}),
    )
    .await;
    let (_, body) = get_usage(&state, "").await;
    assert_eq!(body[0]["outputTokens"], 4);
    assert!(body[0]["hitPercent"].is_null());
}

#[tokio::test]
async fn limit_keeps_the_newest_requests_in_order() {
    let dir = tempdir().unwrap();
    let state = state(&dir).await;
    for input in [100, 200, 300] {
        record(
            &state,
            EventKind::Usage,
            serde_json::json!({"inputTokens": input, "cachedTokens": 0, "outputTokens": 1}),
        )
        .await;
    }
    let (_, body) = get_usage(&state, "?limit=2").await;
    let inputs: Vec<_> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["inputTokens"].as_u64().unwrap())
        .collect();
    assert_eq!(inputs, vec![200, 300]);
}
