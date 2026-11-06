#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::sync::Arc;
use std::time::Duration;

use api::sse::{session_events_handler, EventsQuery};
use api::AppState;
use axum::body::Body;
use axum::extract::Query;
use axum::response::IntoResponse;
use emberloop_core::events::{EventBus, EventKind, SessionEvent};
use futures::StreamExt;
use store::{DataRoot, ProjectsStore, SessionsStore};
use tempfile::tempdir;

async fn state(dir: &tempfile::TempDir) -> AppState {
    let data_root = DataRoot::new(dir.path());
    let projects = ProjectsStore::load(data_root.projects_file())
        .await
        .unwrap();
    let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
    AppState::new(data_root, projects, sessions, Arc::new(EventBus::default()))
}

fn event(seq: u64) -> SessionEvent {
    SessionEvent::new("s1", seq, EventKind::Status, serde_json::json!({}))
}

async fn seqs(body: Body, count: usize) -> Vec<u64> {
    let mut stream = body.into_data_stream();
    let mut text = String::new();
    while text.matches("\n\n").count() < count {
        let chunk = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("stream stalled")
            .expect("stream ended")
            .unwrap();
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|json| {
            serde_json::from_str::<SessionEvent>(json.trim())
                .unwrap()
                .seq
        })
        .collect()
}

#[tokio::test]
async fn replay_then_live_has_no_gap_and_no_duplicate() {
    let dir = tempdir().unwrap();
    let state = state(&dir).await;
    let log = state.get_event_log("s1").await.unwrap();
    for _ in 0..3 {
        log.append(event(0)).await.unwrap();
    }
    let response = session_events_handler(
        state.clone(),
        "s1".into(),
        Query(EventsQuery { after: Some(1) }),
    )
    .await
    .into_response();

    // A duplicate of a replayed event, then a new one.
    state.event_bus.publish(event(3));
    state.event_bus.publish(event(4));

    assert_eq!(seqs(response.into_body(), 3).await, vec![2, 3, 4]);
}

#[tokio::test]
async fn an_unsequenced_event_is_stored_and_delivered_with_its_number() {
    let dir = tempdir().unwrap();
    let state = state(&dir).await;
    state
        .sessions
        .insert(store::SessionMeta {
            id: "s1".into(),
            ..store::SessionMeta::default()
        })
        .await
        .unwrap();
    let _persister = api::persist::spawn_persister(state.clone());
    let response = session_events_handler(
        state.clone(),
        "s1".into(),
        Query(EventsQuery { after: Some(0) }),
    )
    .await
    .into_response();

    state.event_bus.publish(event(0));

    let delivered = seqs(response.into_body(), 1).await;
    assert!(delivered[0] > 0, "{delivered:?}");
    let stored = state.get_event_log("s1").await.unwrap();
    assert_eq!(stored.replay(None).await.unwrap().len(), 1);
}
