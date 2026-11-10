#![allow(clippy::unwrap_used)]
use emberloop_core::steer::{ReviewerReport, SteerQueue, SteerState};

#[tokio::test]
async fn identical_submissions_remain_separate_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("steer_queue.json");
    let queue = SteerQueue::open(&path).await;
    let report = ReviewerReport {
        session_id: "reviewer".into(),
        role: "spec".into(),
        ticket_path: "tickets/one.md".into(),
        status: "done".into(),
        summary: "Missing a requirement".into(),
        report: "The test case is missing.".into(),
    };
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(queue.enqueue_report(report.clone()).await.unwrap().id);
    }
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
    queue.drain_pending().await;
    let restored = SteerQueue::open(path).await.entries().await;
    assert_eq!(restored.len(), 3);
    assert!(restored
        .iter()
        .all(|entry| entry.state == SteerState::Pending));
    assert_eq!(restored[2].report.as_ref(), Some(&report));
    let queues = emberloop_core::steer::SteerQueues::with_dir(dir.path());
    std::fs::create_dir_all(dir.path().join("parent")).unwrap();
    std::fs::copy(
        dir.path().join("steer_queue.json"),
        dir.path().join("parent/steer_queue.json"),
    )
    .unwrap();
    assert!(queues
        .queue("parent")
        .entries()
        .await
        .iter()
        .all(|entry| entry.state == SteerState::Pending));
}

#[tokio::test]
async fn report_is_not_accepted_when_persistence_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-a-directory");
    std::fs::write(&path, "file").unwrap();
    let queue = SteerQueue::open(path.join("queue.json")).await;
    let report = ReviewerReport {
        session_id: "reviewer".into(),
        role: "standards".into(),
        ticket_path: "ticket.md".into(),
        status: "blocked".into(),
        summary: "Blocked".into(),
        report: "Cannot run checks".into(),
    };
    assert!(queue.enqueue_report(report).await.is_err());
    assert!(queue.entries().await.is_empty());
}
