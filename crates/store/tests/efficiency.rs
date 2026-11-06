#![allow(clippy::unwrap_used, clippy::expect_used)]
use emberloop_core::events::{EventKind, SessionEvent};
use std::fs;
use std::time::{Duration, Instant};
use store::archive::ArchivalConfig;
use store::{BlobStore, DataRoot, EventCoalescer, EventLog, SessionMeta, SessionsStore};
use tempfile::tempdir;

fn event(seq: u64, text: &str) -> SessionEvent {
    SessionEvent::new(
        "ses_1",
        seq,
        EventKind::AssistantChunk,
        serde_json::json!({"text": text}),
    )
}

fn notice(seq: u64, text: &str) -> SessionEvent {
    SessionEvent::new(
        "ses_1",
        seq,
        EventKind::Notice,
        serde_json::json!({"text": text}),
    )
}

fn meta(id: &str, updated_at: u64) -> SessionMeta {
    SessionMeta {
        id: id.into(),
        project_id: "proj_1".into(),
        title: "Session".into(),
        ponytail_level: "full".into(),
        created_at: updated_at,
        updated_at,
        prompt_hashes: Default::default(),
        ..SessionMeta::default()
    }
}

#[tokio::test]
async fn blobs_are_content_addressed_and_deduplicated() {
    let dir = tempdir().unwrap();
    let blobs = BlobStore::new(dir.path().join("blobs"));
    let first = blobs.put(b"same bytes").await.unwrap();
    let second = blobs.put(b"same bytes").await.unwrap();

    assert_eq!(first.hash, second.hash);
    assert_eq!(first.size, 10);
    assert_eq!(blobs.get(&first.hash).await.unwrap(), b"same bytes");
    assert_eq!(fs::read_dir(dir.path().join("blobs")).unwrap().count(), 1);
}

#[tokio::test]
async fn tail_seeks_the_last_segment_and_never_parses_the_old_one() {
    let dir = tempdir().unwrap();
    let events_dir = dir.path().join("events");
    let log = EventLog::open_with_segment_bytes(&events_dir, 500)
        .await
        .unwrap();
    for seq in 1..=40 {
        log.append(notice(seq, &"x".repeat(40))).await.unwrap();
    }
    drop(log);

    let mut segments: Vec<_> = fs::read_dir(&events_dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .collect();
    segments.sort();
    assert!(segments.len() > 1, "{segments:?}");
    fs::write(&segments[0], b"not json\n").unwrap();

    let reopened = EventLog::open(&events_dir).await.unwrap();
    assert_eq!(reopened.last_seq().await, 40);
    let tail = reopened.tail(3).await.unwrap();
    assert_eq!(
        tail.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [38, 39, 40]
    );
}

#[tokio::test]
async fn the_ring_buffer_is_bounded() {
    let dir = tempdir().unwrap();
    let log = EventLog::open(dir.path().join("events")).await.unwrap();
    for seq in 1..=250 {
        log.append(event(seq, "chunk")).await.unwrap();
    }
    assert_eq!(log.recent(usize::MAX).await.len(), 200);
    assert_eq!(log.recent(3).await[0].seq, 248);
}

#[tokio::test]
async fn coalescing_turns_many_chunks_into_one_write() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(Duration::from_millis(150));
    let mut due = false;
    for index in 0..100 {
        due = coalescer.push("token ", start + Duration::from_millis(index));
    }
    assert!(!due);
    let text = coalescer.take().unwrap();
    assert_eq!(text.matches("token").count(), 100);
    assert!(coalescer.take().is_none());
}

#[tokio::test]
async fn coalesced_writer_writes_one_event_for_a_burst() {
    let dir = tempdir().unwrap();
    let log = EventLog::open(dir.path().join("events")).await.unwrap();
    let mut writer = store::CoalescedWriter::new(log.clone(), "ses_1", Duration::from_millis(150));
    let start = Instant::now();
    for index in 0..100 {
        writer
            .push("token ", start + Duration::from_millis(index))
            .await
            .unwrap();
    }
    writer.flush().await.unwrap().unwrap();
    let events = log.replay(None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].data["text"]
            .as_str()
            .unwrap()
            .matches("token")
            .count(),
        100
    );
}
#[tokio::test]
async fn session_listing_reads_metadata_only_even_with_a_broken_event_log() {
    let dir = tempdir().unwrap();
    let data_root = DataRoot::new(dir.path());
    let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
    sessions.insert(meta("ses_1", 1_000)).await.unwrap();

    let events_dir = data_root.session_events_dir("ses_1");
    fs::create_dir_all(&events_dir).unwrap();
    fs::write(events_dir.join("000000.jsonl"), b"broken\n").unwrap();

    let reopened = SessionsStore::load(data_root).await.unwrap();
    assert_eq!(reopened.list(None).await.len(), 1);
}

#[test]
fn archival_defaults_match_the_spec() {
    let config = ArchivalConfig::default();
    assert_eq!(config.archive_sweep_hours, 24);
    assert_eq!(config.archive_after_days, 3);
    assert_eq!(config.restore_keep_hours, 24);
}
