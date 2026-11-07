//! Real `claude` turns. Ignored: needs a logged-in CLI, a non-root user (the
//! CLI refuses `bypassPermissions` as root) and the `emberloop-mcp` binary.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use harness::{EventStream, Harness, HarnessEvent, HarnessSession, HarnessWiring};
use harness_claude::ClaudeHarness;
use std::path::PathBuf;
use std::time::Duration;
use tokio_stream::StreamExt;

fn path_env(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set")))
}

/// `LIVE_WORK` is the project root the tools see, `LIVE_MCP` the MCP binary.
fn wiring() -> HarnessWiring {
    let work = path_env("LIVE_WORK");
    let spill = path_env("LIVE_SPILL");
    HarnessWiring {
        proxy_url: "http://127.0.0.1:9/p/tok".into(),
        mcp_url: None,
        mcp_command: Some(vec![
            path_env("LIVE_MCP").display().to_string(),
            "--root".into(),
            work.display().to_string(),
            "--spill".into(),
            spill.display().to_string(),
        ]),
        env: Vec::new(),
        system_prompt: None,
        model: Some("claude/haiku".into()),
        thought_level: Some("low".into()),
        resume_token: None,
        workdir: Some(work.join("agent-work")),
    }
}

async fn live() -> Box<dyn HarnessSession> {
    let harness = ClaudeHarness::new(
        std::env::var("LIVE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
        path_env("LIVE_HOMES"),
    );
    harness.spawn("live", wiring()).await.unwrap()
}

async fn next(events: &mut EventStream) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(120), events.next())
        .await
        .expect("an event within two minutes")
        .expect("the stream is open")
}

/// Every assistant text up to the turn's end.
async fn turn_text(events: &mut EventStream) -> String {
    let mut text = String::new();
    loop {
        match next(events).await {
            HarnessEvent::AssistantChunk(chunk) => text.push_str(&chunk),
            HarnessEvent::TurnFinished { .. } => return text,
            HarnessEvent::Error(error) => panic!("turn failed: {error}"),
            _ => {}
        }
    }
}

#[tokio::test]
#[ignore = "needs a logged-in claude CLI and a non-root user"]
async fn a_haiku_turn_answers_with_text() {
    let session = live().await;
    let mut events = session.events();
    session
        .send("Reply with exactly one word: ready", Vec::new())
        .await
        .unwrap();
    let text = turn_text(&mut events).await;
    assert!(text.to_lowercase().contains("ready"), "{text:?}");
    assert!(session.resume_token().is_some());
}

#[tokio::test]
#[ignore = "needs a logged-in claude CLI and a non-root user"]
async fn a_system_prompt_passed_on_the_command_line_is_followed() {
    let mut wiring = wiring();
    wiring.system_prompt = Some("Always reply with exactly the word PONG.".into());
    let harness = ClaudeHarness::new(
        std::env::var("LIVE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
        path_env("LIVE_HOMES"),
    );
    let session = harness.spawn("live", wiring).await.unwrap();
    let mut events = session.events();
    session.send("ping", Vec::new()).await.unwrap();
    let text = turn_text(&mut events).await;
    assert!(text.contains("PONG"), "{text:?}");
}

#[tokio::test]
#[ignore = "needs a logged-in claude CLI and a non-root user"]
async fn a_message_sent_mid_turn_reaches_the_running_turn() {
    let session = live().await;
    let mut events = session.events();
    session
        .send(
            "Count from 1 to 40, one number per line, with no other text.",
            Vec::new(),
        )
        .await
        .unwrap();
    // The turn is streaming before the steer is written.
    let mut before = String::new();
    while !matches!(next(&mut events).await, HarnessEvent::AssistantChunk(_)) {
        // skip non-text events until the first delta
    }
    let first = match next(&mut events).await {
        HarnessEvent::AssistantChunk(chunk) => chunk,
        _ => String::new(),
    };
    before.push_str(&first);
    session
        .send("Stop. Reply with exactly one word: steered", Vec::new())
        .await
        .unwrap();
    let text = turn_text(&mut events).await;
    assert!(
        text.to_lowercase().contains("steered"),
        "before: {before:?} after: {text:?}"
    );
}
