//! Live skills: one real `claude` turn against the daemon. Ignored: needs a logged-in claude CLI.
//!
//! The daemon runs in this process, so its proxy router carries the turn. Its own binary serves
//! the MCP tools with `--global-skills`, the way a Session's wiring does.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use emberloop_daemon::{Config, Daemon};
use futures::StreamExt;
use harness::{Harness, HarnessEvent, HarnessWiring};
use harness_claude::ClaudeHarness;
use serde_json::{json, Value};
use tower::ServiceExt;

/// A claude config dir that is logged in. Its `.credentials.json` and `.claude.json` are copied
/// into the config dir the harness gives the Session, and nothing else is read.
fn login_home() -> PathBuf {
    PathBuf::from(
        std::env::var("LIVE_CLAUDE_HOME")
            .expect("LIVE_CLAUDE_HOME must name a logged-in claude config dir"),
    )
}

async fn post(daemon: &Daemon, path: &str, body: Value) -> Value {
    let request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = daemon.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// The turn's assistant text and the names of the tools it started, up to the turn's end.
async fn turn(events: &mut harness::EventStream) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut tools = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(240), events.next())
            .await
            .expect("an event within four minutes")
            .expect("the stream is open");
        match event {
            HarnessEvent::AssistantChunk(chunk) => text.push_str(&chunk),
            HarnessEvent::ToolStarted { name, .. } => tools.push(name),
            HarnessEvent::TurnFinished { .. } => return (text, tools),
            HarnessEvent::Error(error) => panic!("turn failed: {error}"),
            _ => {}
        }
    }
}

fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let text = format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n");
    std::fs::write(dir.join("SKILL.md"), text).unwrap();
}

#[tokio::test]
#[ignore = "needs a logged-in claude CLI (LIVE_CLAUDE_HOME, optional LIVE_CLAUDE_BIN)"]
async fn a_claude_turn_searches_a_skill_and_writes_one_through_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let project = base.join("project");
    let global = base.join("global-skills");
    let config_root = base.join("config");
    std::fs::create_dir_all(&project).unwrap();
    write_skill(
        &global,
        "zebra-care",
        "Care notes for zebras",
        "# zebra-care\n\nFeed zebras hay twice a day.",
    );

    let config = Config {
        port: 0,
        host: "127.0.0.1".to_string(),
        data_root: base.join("data"),
        config_root: config_root.clone(),
        global_skills: global.clone(),
        upstream: None,
        allow_remote: false,
    };
    let daemon = Daemon::open(config).await.unwrap();
    let project_id = post(
        &daemon,
        "/api/projects",
        json!({"name": "live", "root": project.display().to_string()}),
    )
    .await;
    let session = post(
        &daemon,
        "/api/sessions",
        json!({"projectId": project_id["id"], "title": "live"}),
    )
    .await;
    let session_id = session["id"].as_str().unwrap().to_string();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = daemon.router();
    tokio::spawn(async move { axum::serve(listener, app).await });

    let homes = base.join("homes");
    let owned = homes.join(&session_id);
    std::fs::create_dir_all(&owned).unwrap();
    for file in [".credentials.json", ".claude.json"] {
        let from = login_home().join(file);
        if from.is_file() {
            std::fs::copy(from, owned.join(file)).unwrap();
        }
    }

    let spill = base.join("spill");
    let wiring = HarnessWiring {
        proxy_url: format!("http://{addr}/proxy/{}", daemon.token_for(&session_id)),
        mcp_url: None,
        mcp_command: Some(vec![
            env!("CARGO_BIN_EXE_emberloop-daemon").to_string(),
            "--config-root".into(),
            config_root.display().to_string(),
            "mcp".into(),
            "--root".into(),
            project.display().to_string(),
            "--spill".into(),
            spill.display().to_string(),
            "--global-skills".into(),
            global.display().to_string(),
        ]),
        env: Vec::new(),
        model: Some("claude/haiku".into()),
        thought_level: Some("low".into()),
        resume_token: None,
        workdir: Some(project.clone()),
    };
    let claude = std::env::var("LIVE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let live = ClaudeHarness::new(claude, homes)
        .spawn(&session_id, wiring)
        .await
        .unwrap();
    let mut events = live.events();
    live.send(
        "Use the skill_search tool to find the skill about zebras. Then use the skill_manage tool \
         to create a skill named zebra-tips with the description 'Short zebra tips' and the body \
         '# zebra-tips' followed by one line of advice. Reply with the name of the skill you found \
         and the path of the one you created.",
        Vec::new(),
    )
    .await
    .unwrap();

    let (text, tools) = turn(&mut events).await;
    assert!(
        tools.iter().any(|name| name.contains("skill_search")),
        "{tools:?}"
    );
    assert!(
        tools.iter().any(|name| name.contains("skill_manage")),
        "{tools:?}"
    );
    assert!(text.contains("zebra-care"), "{text}");
    let created = std::fs::read_to_string(global.join("zebra-tips/SKILL.md")).unwrap();
    assert!(created.contains("origin: agent"), "{created}");
}
