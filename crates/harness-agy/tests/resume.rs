#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::StreamExt;
use harness::{Harness, HarnessEvent, HarnessWiring};
use harness_agy::AgyHarness;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

/// A fake ACP server: `sess-old` loads and replays one old message, any other
/// id is unknown, `session/new` answers `sess-new`, and a prompt answers NEW.
fn fake_server(path: &Path) {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/^{"jsonrpc":"2.0","id":\("[^"]*"\|[0-9][0-9]*\),.*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}\n' "$id";;
    *'"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-new"}}\n' "$id";;
    *'"session/load"'*)
      case "$line" in
        *sess-old*)
          printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-old","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"OLD"}}}}\n'
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id";;
        *)
          printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"unknown session"}}\n' "$id";;
      esac;;
    *'"session/prompt"'*)
      printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"x","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"NEW"}}}}\n'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id";;
    *'"session/set_config_option"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"configOptions":[]}}\n' "$id";;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id";;
  esac
done
"#;
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn wiring(workdir: &Path, token: Option<&str>) -> HarnessWiring {
    HarnessWiring {
        proxy_url: "http://127.0.0.1:9/p/tok".into(),
        mcp_url: None,
        mcp_command: None,
        env: Vec::new(),
        model: None,
        thought_level: None,
        resume_token: token.map(str::to_string),
        workdir: Some(workdir.to_path_buf()),
    }
}

async fn chunks_after_a_prompt(session: &dyn harness::HarnessSession) -> Vec<String> {
    let mut events = session.events();
    session.send("hi", Vec::new()).await.unwrap();
    let mut texts = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(5), events.next()).await {
        match event {
            HarnessEvent::AssistantChunk(text) => texts.push(text),
            HarnessEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }
    texts
}

#[tokio::test]
async fn a_stored_conversation_is_loaded_without_replaying_its_history() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("agy");
    fake_server(&bin);
    let harness = AgyHarness::new(&bin, dir.path().join("home"));
    let workdir = dir.path().join("work");

    let session = harness
        .spawn("s1", wiring(&workdir, Some("sess-old")))
        .await
        .unwrap();

    assert!(session.resumed());
    assert_eq!(session.resume_token().as_deref(), Some("sess-old"));
    assert_eq!(chunks_after_a_prompt(session.as_ref()).await, ["NEW"]);
}

#[tokio::test]
async fn an_unknown_conversation_starts_a_new_one_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("agy");
    fake_server(&bin);
    let harness = AgyHarness::new(&bin, dir.path().join("home"));

    let session = harness
        .spawn("s1", wiring(&dir.path().join("work"), Some("sess-gone")))
        .await
        .unwrap();

    assert!(!session.resumed());
    assert_eq!(session.resume_token().as_deref(), Some("sess-new"));
}

#[tokio::test]
async fn a_first_run_has_a_token_to_store() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("agy");
    fake_server(&bin);
    let harness = AgyHarness::new(&bin, dir.path().join("home"));

    let session = harness
        .spawn("s1", wiring(&dir.path().join("work"), None))
        .await
        .unwrap();

    assert!(!session.resumed());
    assert_eq!(session.resume_token().as_deref(), Some("sess-new"));
}
