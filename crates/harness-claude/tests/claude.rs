//! One Session against the fake CLI: the real process plumbing, no model.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use harness::{EventStream, Harness, HarnessEvent, HarnessWiring};
use harness_claude::ClaudeHarness;
use std::path::Path;
use std::time::Duration;
use tokio_stream::StreamExt;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn wiring(dir: &Path, mode: &str) -> HarnessWiring {
    HarnessWiring {
        proxy_url: "http://127.0.0.1:9/p/tok".into(),
        mcp_url: None,
        mcp_command: Some(vec!["emberloop".into(), "mcp".into()]),
        env: vec![
            ("FAKE_CLAUDE_MODE".into(), mode.into()),
            (
                "FAKE_CLAUDE_LOG".into(),
                dir.join("runs.jsonl").display().to_string(),
            ),
        ],
        model: Some("claude/opus".into()),
        thought_level: None,
        resume_token: None,
        workdir: None,
    }
}

async fn harness(dir: &Path) -> ClaudeHarness {
    ClaudeHarness::new(FAKE, dir.to_path_buf())
}

async fn next(events: &mut EventStream) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .expect("event within the timeout")
        .expect("the stream is open")
}

fn runs(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("runs.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[tokio::test]
async fn the_cli_talks_to_the_proxy_claude_route() {
    let dir = tempfile::tempdir().unwrap();
    let wiring = wiring(dir.path(), "ok");
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring.clone())
        .await
        .unwrap();
    let mut events = session.events();
    session.send("hi", Vec::new()).await.unwrap();
    let _ = next(&mut events).await;
    assert_eq!(
        runs(dir.path())[0]["anthropic_base_url"],
        format!("{}/claude", wiring.proxy_url)
    );
}

#[tokio::test]
async fn a_turn_streams_text_and_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "ok"))
        .await
        .unwrap();
    let mut events = session.events();
    session.send("hi", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("echo:hi".into())
    );
    match next(&mut events).await {
        HarnessEvent::TurnFinished { stop_reason, usage } => {
            assert_eq!(stop_reason.as_deref(), Some("end_turn"));
            assert!(usage.is_some());
        }
        other => panic!("expected TurnFinished, got {other:?}"),
    }
}

/// The real CLI answers a message sent mid-turn as a second turn after the first `result`, so
/// the harness must not report the first result as the end of the work.
#[tokio::test]
async fn a_queued_message_keeps_the_turn_open_until_its_own_result() {
    let dir = tempfile::tempdir().unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "ok"))
        .await
        .unwrap();
    let mut events = session.events();
    session.send("queue:first", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("started".into())
    );
    session.send("second", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("echo:second".into())
    );
    assert!(matches!(
        next(&mut events).await,
        HarnessEvent::TurnFinished { .. }
    ));
    // No result is left over: the next turn's first event is its own text.
    session.send("third", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("echo:third".into())
    );
}

#[tokio::test]
async fn a_message_sent_mid_turn_reaches_the_running_turn() {
    let dir = tempfile::tempdir().unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "ok"))
        .await
        .unwrap();
    let mut events = session.events();
    session.send("wait:first", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("started".into())
    );
    // The turn is blocked on stdin; this write is the steer.
    session.send("second", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("steer:second".into())
    );
    assert!(matches!(
        next(&mut events).await,
        HarnessEvent::TurnFinished { .. }
    ));
}

#[tokio::test]
async fn wake_writes_one_user_message() {
    let dir = tempfile::tempdir().unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "ok"))
        .await
        .unwrap();
    let mut events = session.events();
    session.wake("reminder", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("echo:reminder".into())
    );
    assert!(matches!(
        next(&mut events).await,
        HarnessEvent::TurnFinished { .. }
    ));
}

#[tokio::test]
async fn cancel_ends_the_turn_and_the_session_stays_usable() {
    let dir = tempfile::tempdir().unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "ok"))
        .await
        .unwrap();
    let mut events = session.events();
    session.send("spin:forever", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("started".into())
    );
    session.cancel().await.unwrap();
    match next(&mut events).await {
        HarnessEvent::TurnFinished { stop_reason, .. } => {
            assert_eq!(stop_reason.as_deref(), Some("interrupted"));
        }
        other => panic!("expected the interrupted turn, got {other:?}"),
    }
    session.send("hi", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("echo:hi".into())
    );
}

#[tokio::test]
async fn a_cancel_that_makes_the_cli_report_an_error_and_exit_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "interrupt-exits"))
        .await
        .unwrap();
    let mut events = session.events();
    session.send("spin:forever", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("started".into())
    );
    session.cancel().await.unwrap();
    assert!(matches!(
        next(&mut events).await,
        HarnessEvent::TurnFinished { .. }
    ));
    // The process is gone; the next send starts a new one and nothing reported an error first.
    session.send("hi", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("echo:hi".into())
    );
}

#[tokio::test]
async fn a_crash_is_one_error_and_the_next_send_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "crash-once"))
        .await
        .unwrap();
    let mut events = session.events();
    session.send("dies", Vec::new()).await.unwrap();
    assert!(matches!(next(&mut events).await, HarnessEvent::Error(_)));
    session.send("lives", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("echo:lives".into())
    );
    assert!(session.resumed());
    let runs = runs(dir.path());
    assert_eq!(runs.len(), 2, "one run before the crash and one respawn");
    let respawn: Vec<&str> = runs[1]["args"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|arg| arg.as_str())
        .collect();
    let resume = respawn.iter().position(|arg| *arg == "--resume").unwrap();
    assert_eq!(respawn[resume + 1], "fake-session");
}

#[tokio::test]
async fn a_session_without_its_mcp_server_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "mcp-failed"))
        .await
        .unwrap();
    let error = session
        .send("hi", Vec::new())
        .await
        .expect_err("a Session without el is refused");
    assert!(error.to_string().contains("did not connect"), "{error}");
}

#[tokio::test]
async fn a_session_without_init_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "no-init"))
        .await
        .unwrap();
    let error = session
        .send("hi", Vec::new())
        .await
        .expect_err("a Session without init is refused");
    assert!(
        error.to_string().contains("claude exited before init"),
        "{error}"
    );
}

#[tokio::test]
async fn a_cli_that_dies_on_start_reports_its_last_stderr_line() {
    let dir = tempfile::tempdir().unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "stderr-no-init"))
        .await
        .unwrap();
    let error = session
        .send("hi", Vec::new())
        .await
        .expect_err("a CLI that dies on start fails the turn");
    assert!(
        error.to_string().contains("fake: no auth token"),
        "the CLI's own words reach the error: {error}"
    );
}

#[tokio::test]
async fn a_stored_token_is_loaded_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut wiring = wiring(dir.path(), "ok");
    wiring.resume_token = Some("tok-1".into());
    let session = harness(dir.path()).await.spawn("s1", wiring).await.unwrap();
    session.send("hi", Vec::new()).await.unwrap();
    assert!(session.resumed());
    assert_eq!(session.resume_token().as_deref(), Some("fake-session"));
    let args: Vec<String> = runs(dir.path())[0]["args"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|arg| arg.as_str().map(str::to_string))
        .collect();
    let resume = args.iter().position(|arg| arg == "--resume").unwrap();
    assert_eq!(args[resume + 1], "tok-1");
}

#[tokio::test]
async fn init_can_wait_for_the_first_user_message() {
    let dir = tempfile::tempdir().unwrap();
    let session = tokio::time::timeout(
        Duration::from_secs(1),
        harness(dir.path())
            .await
            .spawn("s1", wiring(dir.path(), "delayed-init")),
    )
    .await
    .expect("spawn must not wait for a prompt")
    .unwrap();
    let mut events = session.events();
    session.send("hi", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("echo:hi".into())
    );
    assert_eq!(session.resume_token().as_deref(), Some("fake-session"));
}

fn creds(token: &str, expires_at: u64) -> String {
    serde_json::json!({"claudeAiOauth": {"accessToken": token, "refreshToken": "r", "expiresAt": expires_at}})
        .to_string()
}

fn expires_at(path: &Path) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value["claudeAiOauth"]["expiresAt"].as_u64())
        .unwrap_or(0)
}

/// The CLI refreshes its own copy in the Session home; the turn end carries it back to the Account.
#[tokio::test]
async fn a_turn_end_pushes_a_fresher_session_credential_to_the_account() {
    let dir = tempfile::tempdir().unwrap();
    let account = dir.path().join(harness_claude::ACCOUNT_HOME);
    std::fs::create_dir_all(&account).unwrap();
    std::fs::write(account.join(".credentials.json"), creds("acc", 1000)).unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "ok"))
        .await
        .unwrap();
    std::fs::write(
        dir.path().join("s1").join(".credentials.json"),
        creds("ses", 2000),
    )
    .unwrap();
    let mut events = session.events();
    session.send("hi", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("echo:hi".into())
    );
    assert!(matches!(
        next(&mut events).await,
        HarnessEvent::TurnFinished { .. }
    ));
    assert_eq!(expires_at(&account.join(".credentials.json")), 2000);
}

/// A respawn starts from the Account's copy when that one is fresher than the Session's.
#[tokio::test]
async fn a_respawn_pulls_a_fresher_account_credential() {
    let dir = tempfile::tempdir().unwrap();
    let account = dir.path().join(harness_claude::ACCOUNT_HOME);
    std::fs::create_dir_all(&account).unwrap();
    std::fs::write(account.join(".credentials.json"), creds("acc", 1000)).unwrap();
    let session = harness(dir.path())
        .await
        .spawn("s1", wiring(dir.path(), "crash-once"))
        .await
        .unwrap();
    let mut events = session.events();
    session.send("dies", Vec::new()).await.unwrap();
    assert!(matches!(next(&mut events).await, HarnessEvent::Error(_)));
    std::fs::write(account.join(".credentials.json"), creds("acc2", 3000)).unwrap();
    session.send("lives", Vec::new()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        HarnessEvent::AssistantChunk("echo:lives".into())
    );
    assert_eq!(
        expires_at(&dir.path().join("s1").join(".credentials.json")),
        3000
    );
}
