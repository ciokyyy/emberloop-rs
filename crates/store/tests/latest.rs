#![allow(clippy::unwrap_used, clippy::expect_used)]
use emberloop_core::events::{EventKind, SessionEvent};
use store::EventLog;
use tempfile::tempdir;

fn ev(kind: EventKind, data: serde_json::Value) -> SessionEvent {
    SessionEvent::new("ses_1", 0, kind, data)
}

#[tokio::test]
async fn latest_finds_the_newest_event_of_a_kind_beyond_the_ring() {
    let dir = tempdir().unwrap();
    let log = EventLog::open(dir.path().join("events")).await.unwrap();
    log.append(ev(
        EventKind::Status,
        serde_json::json!({"state": "working"}),
    ))
    .await
    .unwrap();
    // More than the in-memory ring holds, none of them a status.
    for _ in 0..500 {
        log.append(ev(
            EventKind::ThinkingChunk,
            serde_json::json!({"text": "x"}),
        ))
        .await
        .unwrap();
    }
    let found = log.latest(&EventKind::Status).await.unwrap().unwrap();
    assert_eq!(found.seq, 1);
    assert_eq!(found.data["state"], "working");
}

#[tokio::test]
async fn latest_prefers_the_newest_and_reads_older_segments() {
    let dir = tempdir().unwrap();
    // A tiny segment size forces rotation so the match sits in an older file.
    let log = EventLog::open_with_segment_bytes(dir.path().join("events"), 2048)
        .await
        .unwrap();
    log.append(ev(EventKind::Usage, serde_json::json!({"inputTokens": 10})))
        .await
        .unwrap();
    log.append(ev(EventKind::Usage, serde_json::json!({"inputTokens": 20})))
        .await
        .unwrap();
    for _ in 0..600 {
        log.append(ev(
            EventKind::ThinkingChunk,
            serde_json::json!({"text": "padding padding"}),
        ))
        .await
        .unwrap();
    }
    let reopened = EventLog::open_with_segment_bytes(dir.path().join("events"), 2048)
        .await
        .unwrap();
    let found = reopened.latest(&EventKind::Usage).await.unwrap().unwrap();
    assert_eq!(found.data["inputTokens"], 20);
}

#[tokio::test]
async fn latest_is_none_when_the_kind_never_happened() {
    let dir = tempdir().unwrap();
    let log = EventLog::open(dir.path().join("events")).await.unwrap();
    log.append(ev(EventKind::Prompt, serde_json::json!({})))
        .await
        .unwrap();
    assert!(log.latest(&EventKind::Status).await.unwrap().is_none());
}
