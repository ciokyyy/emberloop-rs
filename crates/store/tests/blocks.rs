#![allow(clippy::unwrap_used, clippy::expect_used)]
use emberloop_core::events::{EventKind, SessionEvent};
use store::EventLog;
use tempfile::tempdir;

fn ev(kind: EventKind, id: Option<&str>) -> SessionEvent {
    let data = id.map_or(serde_json::json!({}), |id| serde_json::json!({ "id": id }));
    SessionEvent::new("ses_1", 0, kind, data)
}

async fn log_of(events: Vec<SessionEvent>) -> (tempfile::TempDir, EventLog) {
    let dir = tempdir().unwrap();
    let log = EventLog::open(dir.path().join("events")).await.unwrap();
    for event in events {
        log.append(event).await.unwrap();
    }
    (dir, log)
}

fn seqs(events: &[SessionEvent]) -> Vec<u64> {
    events.iter().map(|event| event.seq).collect()
}

fn chunks(kind: EventKind, count: usize) -> Vec<SessionEvent> {
    (0..count).map(|_| ev(kind.clone(), None)).collect()
}

#[tokio::test]
async fn a_run_of_chunks_is_one_block() {
    let mut all = vec![ev(EventKind::Prompt, None)];
    all.extend(chunks(EventKind::ThinkingChunk, 3));
    all.extend(chunks(EventKind::AssistantChunk, 5));
    all.push(ev(EventKind::Prompt, None));
    all.extend(chunks(EventKind::AssistantChunk, 2));
    let (_dir, log) = log_of(all).await;

    assert_eq!(seqs(&log.tail(2).await.unwrap()), [10, 11, 12]);
    assert_eq!(
        seqs(&log.tail(3).await.unwrap()),
        [5, 6, 7, 8, 9, 10, 11, 12]
    );
    assert_eq!(
        seqs(&log.page(10, 2).await.unwrap()),
        [2, 3, 4, 5, 6, 7, 8, 9]
    );
}

#[tokio::test]
async fn the_window_starts_on_a_block_boundary() {
    let mut all = vec![ev(EventKind::Prompt, None)];
    all.extend(chunks(EventKind::AssistantChunk, 2));
    all.push(ev(EventKind::Prompt, None));
    all.push(ev(EventKind::AssistantChunk, None));
    let (_dir, log) = log_of(all).await;

    assert_eq!(seqs(&log.tail(2).await.unwrap()), [4, 5]);
}

#[tokio::test]
async fn a_tool_call_is_one_block_even_across_other_events() {
    let all = vec![
        ev(EventKind::ToolStarted, Some("a")),
        ev(EventKind::ToolStarted, Some("a")),
        ev(EventKind::ToolFinished, Some("a")),
        ev(EventKind::AssistantChunk, None),
    ];
    let (_dir, log) = log_of(all).await;

    assert_eq!(seqs(&log.tail(2).await.unwrap()), [1, 2, 3, 4]);
    assert_eq!(seqs(&log.tail(1).await.unwrap()), [4]);
}

#[tokio::test]
async fn a_window_never_cuts_a_tool_from_its_start() {
    let all = vec![
        ev(EventKind::ToolStarted, Some("a")),
        ev(EventKind::ToolStarted, Some("b")),
        ev(EventKind::ToolFinished, Some("a")),
        ev(EventKind::ToolFinished, Some("b")),
        ev(EventKind::AssistantChunk, None),
    ];
    let (_dir, log) = log_of(all).await;

    assert_eq!(seqs(&log.tail(2).await.unwrap()), [1, 2, 3, 4, 5]);
}

#[tokio::test]
async fn status_and_usage_ride_with_the_block_they_sit_in() {
    let all = vec![
        ev(EventKind::Prompt, None),
        ev(EventKind::AssistantChunk, None),
        ev(EventKind::Usage, None),
        ev(EventKind::Status, None),
    ];
    let (_dir, log) = log_of(all).await;

    assert_eq!(seqs(&log.tail(1).await.unwrap()), [2, 3, 4]);
}
