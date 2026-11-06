#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::StreamExt;
use harness::{Harness, HarnessEvent, HarnessWiring};
use harness_pi::PiHarness;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn wiring() -> HarnessWiring {
    HarnessWiring {
        proxy_url: "http://127.0.0.1:9/p/tok".into(),
        mcp_url: None,
        mcp_command: Some(vec![
            "emberloop".into(),
            "mcp".into(),
            "--root".into(),
            "/p".into(),
        ]),
        env: Vec::new(),
        model: Some("opencode-go/glm-5".into()),
        thought_level: None,
        resume_token: None,
        workdir: None,
    }
}

fn harness(dir: &Path, with_key: bool) -> PiHarness {
    let keys = dir.join("keys.json");
    if with_key {
        std::fs::write(&keys, r#"{"opencode-go":"sk-test"}"#).unwrap();
    }
    PiHarness::new(env!("CARGO_BIN_EXE_fake-pi"), dir.join("pi"), keys)
}

async fn report(harness: &PiHarness) -> serde_json::Value {
    let session = harness.spawn("s1", wiring()).await.unwrap();
    let mut events = session.events();
    session.send("hi", Vec::new()).await.unwrap();
    while let Some(event) = events.next().await {
        if let HarnessEvent::AssistantChunk(text) = event {
            return serde_json::from_str(&text).unwrap();
        }
    }
    panic!("no report");
}

#[tokio::test]
async fn pi_starts_with_the_confined_arguments_and_a_scrubbed_environment() {
    std::env::set_var("OPENCODE_API_KEY", "leaked");
    let dir = tempfile::tempdir().unwrap();
    let report = report(&harness(dir.path(), true)).await;
    assert_eq!(
        report["args"],
        serde_json::json!(["--mode", "acp", "--no-builtin-tools", "--offline"])
    );
    let mut env: Vec<String> = serde_json::from_value(report["env"].clone()).unwrap();
    env.sort();
    assert_eq!(env, ["HOME", "PATH"]);
    let picked = report["picked"].as_str().unwrap();
    assert!(
        picked.starts_with("model=") && picked.contains("opencode-go/glm-5"),
        "{picked}"
    );
}

#[tokio::test]
async fn the_session_home_points_pi_at_the_proxy_and_the_tools() {
    let dir = tempfile::tempdir().unwrap();
    let report = report(&harness(dir.path(), true)).await;
    let models: serde_json::Value =
        serde_json::from_str(report["models"].as_str().unwrap()).unwrap();
    assert_eq!(
        models["providers"]["opencode-go"]["baseUrl"],
        "http://127.0.0.1:9/p/tok/v1"
    );
    assert_eq!(models["providers"]["opencode-go"]["apiKey"], "sk-test");
    let mcp: serde_json::Value = serde_json::from_str(report["mcp"].as_str().unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["el"]["command"], "emberloop");
    assert_eq!(mcp["mcpServers"]["el"]["args"][0], "mcp");
    assert_eq!(mcp["mcpServers"]["el"]["exposure"], "direct");
    let mode = std::fs::metadata(dir.path().join("pi/s1/.pi/agent/models.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(
        report["cwd"],
        dir.path().join("pi/s1/work").display().to_string()
    );

    let mut custom = wiring();
    let custom_dir = dir.path().join("agent-work");
    custom.workdir = Some(custom_dir.clone());
    let session = harness(dir.path(), true).spawn("s2", custom).await.unwrap();
    let mut events = session.events();
    session.send("hi", Vec::new()).await.unwrap();
    while let Some(event) = events.next().await {
        if let HarnessEvent::AssistantChunk(text) = event {
            let custom_report: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(custom_report["cwd"], custom_dir.display().to_string());
            break;
        }
    }
}

#[tokio::test]
async fn an_http_mcp_url_wins_over_the_stdio_command() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path(), true);
    let mut wiring = wiring();
    wiring.mcp_url = Some("http://127.0.0.1:9/mcp".into());
    let session = harness.spawn("s1", wiring).await.unwrap();
    drop(session);
    let mcp: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("pi/s1/.pi/agent/mcp.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(mcp["mcpServers"]["el"]["url"], "http://127.0.0.1:9/mcp");
    assert!(mcp["mcpServers"]["el"]["command"].is_null());
}

#[tokio::test]
async fn a_missing_key_is_refused_before_pi_starts() {
    let dir = tempfile::tempdir().unwrap();
    let error = harness(dir.path(), false)
        .spawn("s1", wiring())
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("keys.json"), "{error}");
    assert!(!dir.path().join("pi/s1").exists());
}

#[tokio::test]
async fn forget_deletes_the_session_in_pi() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path(), true);
    let session = harness.spawn("s1", wiring()).await.unwrap();
    let mut events = session.events();
    session.forget("pi-1").await.unwrap();
    while let Some(event) = events.next().await {
        if matches!(event, HarnessEvent::AssistantChunk(text) if text == "deleted pi-1") {
            return;
        }
    }
    panic!("pi never saw session/delete");
}

#[tokio::test]
async fn the_catalogue_is_the_model_option_pi_returns_from_session_new() {
    let dir = tempfile::tempdir().unwrap();
    let models = harness(dir.path(), true).models().await.unwrap();
    assert_eq!(
        models,
        [
            ("opencode-go/glm-5".to_string(), "GLM-5".to_string()),
            ("opencode-go/kimi-k2".to_string(), "KIMI-K2".to_string()),
        ]
    );
}

#[test]
fn grouped_options_flatten_and_other_options_are_ignored() {
    let options = serde_json::json!([
        {"id": "mode", "name": "Mode", "type": "select", "currentValue": "auto",
         "options": [{"value": "auto", "name": "Auto"}]},
        {"id": "model", "name": "Model", "type": "select", "currentValue": "a/x",
         "options": [{"group": "g", "name": "G", "options": [{"value": "a/x", "name": "X"}]}]},
    ]);
    assert_eq!(
        harness_pi::model_options(&options),
        [("a/x".to_string(), "X".to_string())]
    );
    assert!(harness_pi::model_options(&serde_json::Value::Null).is_empty());
}
