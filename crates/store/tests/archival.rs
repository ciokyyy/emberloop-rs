#![allow(clippy::unwrap_used, clippy::expect_used)]
use emberloop_core::events::{EventKind, SessionEvent};
use store::archive::{ArchivalConfig, ArchivalSweeper};
use store::{DataRoot, EventLog, SessionMeta, SessionState, SessionsStore};
use tempfile::tempdir;

const HOUR: u64 = 60 * 60 * 1000;
const DAY: u64 = 24 * HOUR;

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

async fn append(data_root: &DataRoot, id: &str, text: &str) {
    let log = EventLog::open(data_root.session_events_dir(id))
        .await
        .unwrap();
    log.append(SessionEvent::new(
        id,
        0,
        EventKind::Prompt,
        serde_json::json!({"text": text}),
    ))
    .await
    .unwrap();
}

async fn setup(last_used_at: u64) -> (tempfile::TempDir, DataRoot, SessionsStore, ArchivalSweeper) {
    let dir = tempdir().unwrap();
    let data_root = DataRoot::new(dir.path());
    let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
    sessions.insert(meta("ses_1", last_used_at)).await.unwrap();
    append(&data_root, "ses_1", "old prompt").await;
    let sweeper = ArchivalSweeper::new(
        data_root.clone(),
        sessions.clone(),
        ArchivalConfig::default(),
    );
    (dir, data_root, sessions, sweeper)
}

#[tokio::test]
async fn active_session_archives_restores_and_resumes() {
    let now = 10 * DAY;
    let (_dir, data_root, sessions, sweeper) = setup(now - 4 * DAY).await;
    let events_dir = data_root.session_events_dir("ses_1");
    let archive = data_root.session_archive_file("ses_1");

    sweeper.sweep_at(now).await.unwrap();
    assert_eq!(
        sessions.get("ses_1").await.unwrap().state,
        SessionState::Archived
    );
    assert!(archive.exists());
    assert!(!events_dir.exists());

    sweeper.restore_at("ses_1", now).await.unwrap();
    let restored = sessions.get("ses_1").await.unwrap();
    assert_eq!(restored.state, SessionState::Restored);
    assert_eq!(restored.restored_at, Some(now));
    assert_eq!(restored.last_used_at, Some(now));
    assert!(archive.exists());
    let log = EventLog::open(&events_dir).await.unwrap();
    assert_eq!(log.replay(None).await.unwrap().len(), 1);

    append(&data_root, "ses_1", "new prompt").await;
    sweeper.sweep_at(now + 25 * HOUR).await.unwrap();
    let resumed = sessions.get("ses_1").await.unwrap();
    assert_eq!(resumed.state, SessionState::Active);
    assert!(events_dir.exists());
    assert!(!archive.exists());
}

#[tokio::test]
async fn a_restored_session_that_is_only_viewed_loses_its_hot_copy() {
    let now = 10 * DAY;
    let (_dir, data_root, sessions, sweeper) = setup(now - 4 * DAY).await;
    let events_dir = data_root.session_events_dir("ses_1");
    let archive = data_root.session_archive_file("ses_1");

    sweeper.sweep_at(now).await.unwrap();
    sweeper.restore_at("ses_1", now).await.unwrap();
    sweeper.sweep_at(now + 25 * HOUR).await.unwrap();

    let meta = sessions.get("ses_1").await.unwrap();
    assert_eq!(meta.state, SessionState::Archived);
    assert!(!events_dir.exists());
    assert!(archive.exists());
}

#[tokio::test]
async fn a_second_sweep_over_an_archived_session_changes_nothing() {
    let now = 10 * DAY;
    let (_dir, data_root, sessions, sweeper) = setup(now - 4 * DAY).await;
    let archive = data_root.session_archive_file("ses_1");

    sweeper.sweep_at(now).await.unwrap();
    let before = std::fs::read(&archive).unwrap();
    let report = sweeper.sweep_at(now).await.unwrap();
    assert!(report.archived.is_empty());
    assert_eq!(std::fs::read(&archive).unwrap(), before);
    assert_eq!(
        sessions.get("ses_1").await.unwrap().state,
        SessionState::Archived
    );
}

#[tokio::test]
async fn the_scheduler_sweeps_at_boot() {
    let (_dir, _data_root, sessions, sweeper) = setup(1).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let handle = sweeper.spawn_scheduler(move |report| {
        let _ = tx.send(report);
    });

    let report = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("no sweep at boot")
        .unwrap()
        .unwrap();
    handle.abort();
    assert_eq!(report.archived, vec!["ses_1".to_string()]);
    assert_eq!(
        sessions.get("ses_1").await.unwrap().state,
        SessionState::Archived
    );
}
