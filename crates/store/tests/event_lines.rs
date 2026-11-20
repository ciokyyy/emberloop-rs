#![allow(clippy::unwrap_used, clippy::expect_used)]
use emberloop_core::events::{EventKind, SessionEvent};
use store::archive::{ArchivalConfig, ArchivalSweeper};
use store::{DataRoot, EventLog, SessionMeta, SessionsStore};
use tempfile::tempdir;

const DAY: u64 = 24 * 60 * 60 * 1000;

fn meta(id: &str, last_used_at: u64) -> SessionMeta {
    SessionMeta {
        id: id.into(),
        project_id: "proj_1".into(),
        title: "Old Session".into(),
        created_at: last_used_at,
        updated_at: last_used_at,
        last_used_at: Some(last_used_at),
        ..SessionMeta::default()
    }
}

#[tokio::test]
async fn reads_a_live_and_an_archived_session() {
    let now = 10 * DAY;
    let dir = tempdir().unwrap();
    let data_root = DataRoot::new(dir.path());
    let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
    sessions.insert(meta("ses_1", now - 4 * DAY)).await.unwrap();
    let log = EventLog::open(data_root.session_events_dir("ses_1"))
        .await
        .unwrap();
    log.append(SessionEvent::new(
        "ses_1",
        0,
        EventKind::Prompt,
        serde_json::json!({"text": "live prompt"}),
    ))
    .await
    .unwrap();

    let live = store::session_event_lines(&data_root, "ses_1");
    assert_eq!(live.len(), 1, "{live:?}");
    assert!(live[0].contains("live prompt"), "{live:?}");

    let sweeper = ArchivalSweeper::new(
        data_root.clone(),
        sessions.clone(),
        ArchivalConfig::default(),
    );
    sweeper.sweep_at(now).await.unwrap();
    assert!(!data_root.session_events_dir("ses_1").exists());

    let archived = store::session_event_lines(&data_root, "ses_1");
    assert_eq!(archived, live);
}
