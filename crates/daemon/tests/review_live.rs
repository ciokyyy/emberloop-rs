//! Live background skill review: a real `claude` Session, the proxy forks a review after a final
//! answer. Ignored: needs a logged-in claude CLI.
//!
//! It prints the main requests' cache figures (the proxy's `usage` events) and the fork's
//! (`review.jsonl`), and asserts that the fork reads the main prefix from the cache and that the
//! next main request still does.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
// A live test waits on a real model, so a real sleep is the point here.
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request};
use emberloop_daemon::{Config, Daemon};
use futures::StreamExt;
use harness::{Harness, HarnessEvent, HarnessWiring};
use harness_claude::ClaudeHarness;
use harness_pi::PiHarness;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(daemon: &Daemon, method: Method, path: &str, body: Value) -> Value {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = daemon.router().oneshot(request).await.unwrap();
    assert!(
        response.status().is_success(),
        "{path}: {}",
        response.status()
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn turn(events: &mut harness::EventStream) {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(240), events.next())
            .await
            .expect("an event within four minutes")
            .expect("the stream is open");
        match event {
            HarnessEvent::TurnFinished { .. } => return,
            HarnessEvent::Error(error) => panic!("turn failed: {error}"),
            _ => {}
        }
    }
}

fn lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// `(input, cached, created)` of every `usage` event of the Session, oldest first.
fn main_usage(session_dir: &std::path::Path) -> Vec<(u64, u64, u64)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(session_dir.join("events"))
        .unwrap()
        .flatten()
    {
        if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
            for event in lines(&entry.path()) {
                if event["kind"] == "usage" {
                    let data = &event["data"];
                    found.push((
                        data["inputTokens"].as_u64().unwrap_or(0),
                        data["cachedTokens"].as_u64().unwrap_or(0),
                        data["cacheCreationTokens"].as_u64().unwrap_or(0),
                    ));
                }
            }
        }
    }
    found
}

#[tokio::test]
#[ignore = "needs a logged-in claude CLI (LIVE_CLAUDE_HOME, optional LIVE_CLAUDE_BIN)"]
async fn the_background_review_leaves_the_main_prompt_cache_intact() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let project = base.join("project");
    let global = base.join("global-skills");
    let config_root = base.join("config");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("notes.txt"), "alpha\nbeta\ngamma\n").unwrap();

    let config = Config {
        idle_timeout: emberloop_daemon::idle::DEFAULT_IDLE_TIMEOUT,
        port: 0,
        host: "127.0.0.1".to_string(),
        data_root: base.join("data"),
        config_root: config_root.clone(),
        global_skills: global.clone(),
        upstream: None,
        allow_remote: false,
    };
    let daemon = Daemon::open(config).await.unwrap();
    // The default interval is 5 rounds, so the prompt below asks for six separate reads.
    let project_id = call(
        &daemon,
        Method::POST,
        "/api/projects",
        json!({"name": "live", "root": project.display().to_string()}),
    )
    .await;
    let session = call(
        &daemon,
        Method::POST,
        "/api/sessions",
        json!({"projectId": project_id["id"], "title": "live"}),
    )
    .await;
    let session_id = session["id"].as_str().unwrap().to_string();
    let session_dir = base.join("data/sessions").join(&session_id);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pi = std::env::var("LIVE_HARNESS").is_ok_and(|name| name == "pi");
    let addr = listener.local_addr().unwrap();
    let app = daemon.router();
    tokio::spawn(async move { axum::serve(listener, app).await });

    let homes = base.join("homes");
    let owned = homes.join(&session_id);
    std::fs::create_dir_all(&owned).unwrap();
    if !pi {
        let login = PathBuf::from(std::env::var("LIVE_CLAUDE_HOME").expect("LIVE_CLAUDE_HOME"));
        for file in [".credentials.json", ".claude.json"] {
            if login.join(file).is_file() {
                std::fs::copy(login.join(file), owned.join(file)).unwrap();
            }
        }
    }

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
            base.join("spill").display().to_string(),
            "--global-skills".into(),
            global.display().to_string(),
        ]),
        env: Vec::new(),
        model: Some(
            if pi {
                "opencode-go/deepseek-v4.1-flash"
            } else {
                "claude/haiku"
            }
            .into(),
        ),
        thought_level: if pi { None } else { Some("low".into()) },
        resume_token: None,
        workdir: Some(project.clone()),
    };
    let live = if pi {
        let keys = std::env::var("LIVE_PI_KEYS").expect("LIVE_PI_KEYS names keys.json");
        PiHarness::new(
            std::env::var("LIVE_PI_BIN").expect("LIVE_PI_BIN names the pi binary"),
            base.join("pi-homes"),
            keys,
        )
        .spawn(&session_id, wiring)
        .await
        .unwrap()
    } else {
        let claude = std::env::var("LIVE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
        ClaudeHarness::new(claude, homes)
            .spawn(&session_id, wiring)
            .await
            .unwrap()
    };
    let mut events = live.events();

    live.send(
        "Call the read tool on notes.txt six times, one call per step and never in parallel. Then reply with its first line only.",
        Vec::new(),
    )
    .await
    .unwrap();
    turn(&mut events).await;

    // The fork runs after the final answer; wait for its last round to be logged.
    let log = session_dir.join("review.jsonl");
    for _ in 0..120 {
        let rounds = lines(&log);
        if rounds.last().is_some_and(|round| round["calls"] == 0) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    live.send("Reply with the word done.", Vec::new())
        .await
        .unwrap();
    turn(&mut events).await;

    let main = main_usage(&session_dir);
    let fork = lines(&log);
    eprintln!("main (input, cached, created): {main:?}");
    eprintln!("fork: {fork:#?}");
    assert!(!fork.is_empty(), "the review never ran");
    assert!(
        fork[0]["cached"].as_u64().unwrap() > 0,
        "the fork read nothing from the main cache: {}",
        fork[0]
    );
    let last = main.last().unwrap();
    assert!(
        last.1 > 0,
        "the next main request read nothing from the cache: {main:?}"
    );
}
