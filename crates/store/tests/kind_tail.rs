#![allow(clippy::unwrap_used, clippy::expect_used)]
use emberloop_core::events::{EventKind, SessionEvent};
use store::EventLog;
use tempfile::tempdir;

fn ev(kind: EventKind, data: serde_json::Value) -> SessionEvent {
    SessionEvent::new("ses_1", 0, kind, data)
}

#[tokio::test]
async fn kind_tail_returns_the_newest_n_of_one_kind_oldest_first_across_segments() {
    let dir = tempdir().unwrap();
    // A tiny segment size spreads the usage events over several files.
    let log = EventLog::open_with_segment_bytes(dir.path().join("events"), 2048)
        .await
        .unwrap();
    for n in 1..=5u64 {
        log.append(ev(EventKind::Usage, serde_json::json!({"inputTokens": n})))
            .await
            .unwrap();
        for _ in 0..40 {
            log.append(ev(
                EventKind::ThinkingChunk,
                serde_json::json!({"text": "padding padding"}),
            ))
            .await
            .unwrap();
        }
    }
    // More than the in-memory ring holds after the last usage event.
    for _ in 0..300 {
        log.append(ev(
            EventKind::ThinkingChunk,
            serde_json::json!({"text": "x"}),
        ))
        .await
        .unwrap();
    }
    let found = log.kind_tail(&EventKind::Usage, 3).await.unwrap();
    let tokens: Vec<u64> = found
        .iter()
        .map(|event| event.data["inputTokens"].as_u64().unwrap())
        .collect();
    assert_eq!(tokens, vec![3, 4, 5]);
    assert!(found.iter().all(|event| event.kind == EventKind::Usage));
    assert!(found.windows(2).all(|pair| pair[0].seq < pair[1].seq));
}

#[tokio::test]
async fn kind_tail_returns_everything_of_the_kind_when_fewer_exist() {
    let dir = tempdir().unwrap();
    let log = EventLog::open(dir.path().join("events")).await.unwrap();
    log.append(ev(EventKind::Usage, serde_json::json!({"inputTokens": 1})))
        .await
        .unwrap();
    log.append(ev(EventKind::Prompt, serde_json::json!({})))
        .await
        .unwrap();
    log.append(ev(EventKind::Usage, serde_json::json!({"inputTokens": 2})))
        .await
        .unwrap();
    let found = log.kind_tail(&EventKind::Usage, 10).await.unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].data["inputTokens"], 1);
    assert_eq!(found[1].data["inputTokens"], 2);
}

#[tokio::test]
async fn kind_tail_is_empty_for_a_zero_limit_or_an_absent_kind() {
    let dir = tempdir().unwrap();
    let log = EventLog::open(dir.path().join("events")).await.unwrap();
    log.append(ev(EventKind::Usage, serde_json::json!({"inputTokens": 1})))
        .await
        .unwrap();
    assert!(log
        .kind_tail(&EventKind::Usage, 0)
        .await
        .unwrap()
        .is_empty());
    assert!(log
        .kind_tail(&EventKind::Status, 5)
        .await
        .unwrap()
        .is_empty());
}
