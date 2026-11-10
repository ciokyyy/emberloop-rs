#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::path::Path;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::routing::any;
use axum::{Json, Router};
use emberloop_daemon::{Config, Daemon};
use serde_json::{json, Value};
use store::{ProjectsStore, SessionsStore};
use tempfile::TempDir;
use tower::ServiceExt;

struct Harness {
    _dir: TempDir,
    daemon: Daemon,
}

async fn harness(upstream: Option<String>) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        port: 0,
        host: "127.0.0.1".to_string(),
        data_root: dir.path().join("data"),
        config_root: dir.path().join("config"),
        global_skills: dir.path().join("home/.agents/skills"),
        upstream,
        allow_remote: false,
        idle_timeout: emberloop_daemon::idle::DEFAULT_IDLE_TIMEOUT,
    };
    let daemon = Daemon::open(config).await.unwrap();
    Harness { _dir: dir, daemon }
}

async fn fake_upstream() -> (String, Arc<tokio::sync::Mutex<Vec<Value>>>) {
    let bodies = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let state = bodies.clone();
    let app =
        Router::new()
            .fallback(any(
                |State(bodies): State<Arc<tokio::sync::Mutex<Vec<Value>>>>,
                 Json(body): Json<Value>| async move {
                    bodies.lock().await.push(body);
                    Json(json!({
                        "candidates": [],
                        "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 2}
                    }))
                },
            ))
            .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (url, bodies)
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn project_and_session(daemon: &Daemon, root: &Path) -> (String, String) {
    let (status, project) = request(
        &daemon.router(),
        Method::POST,
        "/api/projects",
        Some(json!({"name": "demo", "root": root.display().to_string()})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    let project_id = project["id"].as_str().unwrap().to_string();

    let (status, session) = request(
        &daemon.router(),
        Method::POST,
        "/api/sessions",
        Some(json!({"projectId": project_id, "title": "first"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{session}");
    (project_id, session["id"].as_str().unwrap().to_string())
}

fn generation_body(text: &str) -> Value {
    json!({"contents": [{"role": "user", "parts": [{"text": text}]}]})
}

fn system_text(body: &Value) -> String {
    let root = body.get("request").unwrap_or(body);
    root.pointer("/systemInstruction/parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn acceptance_1_project_and_session_persist() {
    let env = harness(None).await;
    let root = env._dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let (project_id, session_id) = project_and_session(&env.daemon, &root).await;

    assert!(env.daemon.data_root.session_meta_file(&session_id).exists());
    let projects = ProjectsStore::load(env.daemon.data_root.projects_file())
        .await
        .unwrap();
    let sessions = SessionsStore::load(env.daemon.data_root.clone())
        .await
        .unwrap();
    assert_eq!(projects.get(&project_id).await.unwrap().name, "demo");
    let meta = sessions.get(&session_id).await.unwrap();
    assert_eq!(meta.title, "first");
    assert_eq!(meta.project_id, project_id);
}

#[tokio::test]
async fn acceptance_2_proxy_rejects_unsigned_and_tampered_tokens() {
    let (upstream, bodies) = fake_upstream().await;
    let env = harness(Some(upstream)).await;
    let root = env._dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let (_, session_id) = project_and_session(&env.daemon, &root).await;
    let app = env.daemon.router();

    let (status, _) = request(
        &app,
        Method::POST,
        "/proxy/not-a-token/v1internal:generateContent",
        Some(generation_body("hi")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let token = env.daemon.token_for(&session_id);
    let (status, _) = request(
        &app,
        Method::POST,
        &format!("/proxy/{token}x/v1internal:generateContent"),
        Some(generation_body("hi")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = request(
        &app,
        Method::POST,
        &format!("/proxy/{token}/v1internal:generateContent"),
        Some(generation_body("hi")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bodies.lock().await.len(), 1);
}

#[tokio::test]
async fn acceptance_7_prompt_pins_survive_a_config_edit_but_new_sessions_see_it() {
    let (upstream, bodies) = fake_upstream().await;
    let env = harness(Some(upstream)).await;
    let root = env._dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let (_, running) = project_and_session(&env.daemon, &root).await;
    let app = env.daemon.router();

    let token = env.daemon.token_for(&running);
    let (status, _) = request(
        &app,
        Method::POST,
        &format!("/proxy/{token}/v1internal:generateContent"),
        Some(generation_body("before")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let prompt_file = env.daemon.config().config_root.join("prompts/ponytail.md");
    std::fs::create_dir_all(prompt_file.parent().unwrap()).unwrap();
    std::fs::write(&prompt_file, "CUSTOM-PONYTAIL-E2E\n").unwrap();
    env.daemon.reload_prompts();

    let (status, _) = request(
        &app,
        Method::POST,
        &format!("/proxy/{token}/v1internal:generateContent"),
        Some(generation_body("after")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, new_session) = request(
        &app,
        Method::POST,
        "/api/sessions",
        Some(json!({"projectId": env.daemon.sessions.get(&running).await.unwrap().project_id, "title": "new"})),
    )
    .await;
    let new_session = new_session["id"].as_str().unwrap().to_string();
    let new_token = env.daemon.token_for(&new_session);
    let (status, _) = request(
        &app,
        Method::POST,
        &format!("/proxy/{new_token}/v1internal:generateContent"),
        Some(generation_body("new")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 3);
    assert!(!system_text(&bodies[0]).contains("CUSTOM-PONYTAIL-E2E"));
    assert!(!system_text(&bodies[1]).contains("CUSTOM-PONYTAIL-E2E"));
    assert!(system_text(&bodies[2]).contains("CUSTOM-PONYTAIL-E2E"));

    let old_hash = env
        .daemon
        .sessions
        .get(&running)
        .await
        .unwrap()
        .prompt_hashes
        .get("ponytail")
        .cloned();
    let new_hash = env
        .daemon
        .sessions
        .get(&new_session)
        .await
        .unwrap()
        .prompt_hashes
        .get("ponytail")
        .cloned();
    assert!(old_hash.is_some() && new_hash.is_some());
    assert_ne!(old_hash, new_hash);
}

#[tokio::test]
async fn acceptance_8_background_process_is_adopted_after_restart() {
    let env = harness(None).await;
    let root = env._dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let (_, session_id) = project_and_session(&env.daemon, &root).await;

    let toolbox = env.daemon.toolbox_for(&session_id).await.unwrap().unwrap();
    let started = toolbox.background().spawn("sleep 30", "").await.unwrap();
    let id = started
        .split_whitespace()
        .find(|word| word.starts_with("bg_"))
        .unwrap()
        .to_string();
    drop(toolbox);

    // Tool descriptions come from the Prompt Store, not the hardcoded specs.
    let config = env.daemon.config().clone();
    std::fs::create_dir_all(config.config_root.join("prompts")).unwrap();
    std::fs::write(
        config
            .config_root
            .join("prompts/tool.execute.description.md"),
        "OVERRIDE-EXECUTE-DESCRIPTION",
    )
    .unwrap();
    let restarted = Daemon::open(config).await.unwrap();
    let toolbox = restarted.toolbox_for(&session_id).await.unwrap().unwrap();
    let listed = toolbox
        .background()
        .run(&json!({"action": "list"}))
        .await
        .unwrap();
    assert!(listed.contains(&id), "{listed}");
    assert!(
        listed.contains("running") || listed.contains("exited"),
        "{listed}"
    );
    let read = toolbox
        .background()
        .run(&json!({"action": "read", "id": id}))
        .await
        .unwrap();
    assert!(read.contains("next offset"), "{read}");
    toolbox
        .background()
        .run(&json!({"action": "stop", "id": id}))
        .await
        .unwrap();

    let mcp = restarted
        .mcp_server_for(&session_id)
        .await
        .unwrap()
        .unwrap();
    let names: Vec<String> = mcp
        .tools()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(names.contains(&"read".to_string()), "{names:?}");
    assert!(names.contains(&"background".to_string()), "{names:?}");
    let execute = mcp
        .tools()
        .into_iter()
        .find(|tool| tool.name == "execute")
        .unwrap();
    assert_eq!(
        execute.description.as_deref(),
        Some("OVERRIDE-EXECUTE-DESCRIPTION")
    );
}
#[tokio::test]
async fn spa_routes_fall_back_to_the_embedded_index() {
    let env = harness(None).await;
    let app = env.daemon.router();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("<html"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/project-1/s/session-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("<html"));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn remote_bind_requires_the_explicit_flag() {
    let config = Config {
        host: "0.0.0.0".to_string(),
        ..Config::default()
    };
    assert!(config.validate().is_err());
    let config = Config {
        allow_remote: true,
        ..config
    };
    assert!(config.validate().is_ok());
    let config = Config {
        host: "localhost".to_string(),
        allow_remote: false,
        ..Config::default()
    };
    assert!(config.validate().is_ok());
}

#[tokio::test]
async fn harness_wiring_carries_a_signed_proxy_url_and_mcp_command() {
    let env = harness(None).await;
    let root = env._dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let (_, session_id) = project_and_session(&env.daemon, &root).await;
    let wiring = env
        .daemon
        .harness_wiring(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert!(wiring.proxy_url.contains("/proxy/"), "{}", wiring.proxy_url);
    assert!(wiring
        .proxy_url
        .contains(&env.daemon.token_for(&session_id)));
    let command = wiring.mcp_command.unwrap();
    assert!(command.iter().any(|arg| arg == "mcp"));
    assert!(command.iter().any(|arg| arg == "--config-root"));
    assert!(command.iter().any(|arg| arg == "--root"));
    assert!(command.iter().any(|arg| arg == "--spill"));
    assert!(command.iter().any(|arg| arg == "--global-skills"));
}

#[tokio::test]
async fn pi_sessions_and_catalogue_run_on_the_pi_harness_without_an_agy_account() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let fake_pi = dir.path().join("fake-pi");
    let script = "#!/bin/sh\n\
        while IFS= read -r line; do\n\
          id=$(printf '%s' \"$line\" | sed -n 's/^{\"jsonrpc\":\"2.0\",\"id\":\\([^,}]*\\).*/\\1/p')\n\
          rpcid=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\(\"[^\"]*\"\\).*/\\1/p')\n\
          case \"$line\" in\n\
            *\"get_available_models\"*)\n\
              printf '{\"id\":%s,\"success\":true,\"data\":{\"models\":[{\"id\":\"glm-5\",\"provider\":\"opencode-go\",\"name\":\"GLM-5\",\"reasoning\":true,\"input\":[\"text\",\"image\"],\"contextWindow\":200000}]}}\\n' \"$rpcid\"\n\
              ;;\n\
            *\"get_available_thinking_levels\"*)\n\
              printf '{\"id\":%s,\"success\":true,\"data\":{\"levels\":[\"low\",\"high\"]}}\\n' \"$rpcid\"\n\
              ;;\n\
            *\"set_model\"*)\n\
              printf '{\"id\":%s,\"success\":true,\"data\":{}}\\n' \"$rpcid\"\n\
              ;;\n\
            *\"initialize\"*)\n\
              printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{}}}\\n' \"$id\"\n\
              ;;\n\
            *\"session/new\"*)\n\
              printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"sessionId\":\"pi-s1\",\"configOptions\":[{\"id\":\"model\",\"name\":\"Model\",\"type\":\"select\",\"currentValue\":\"opencode-go/glm-5\",\"options\":[{\"value\":\"opencode-go/glm-5\",\"name\":\"opencode-go: GLM-5\"}]}]}}\\n' \"$id\"\n\
              ;;\n\
            *\"session/set_config_option\"*)\n\
              printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"configOptions\":[]}}\\n' \"$id\"\n\
              ;;\n\
            *\"session/prompt\"*)\n\
              printf '{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"pi-s1\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"from-pi\"}}}}\\n'\n\
              printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"stopReason\":\"end_turn\"}}\\n' \"$id\"\n\
              ;;\n\
          esac\n\
        done\n";
    std::fs::write(&fake_pi, script).unwrap();
    std::fs::set_permissions(&fake_pi, std::fs::Permissions::from_mode(0o755)).unwrap();

    let data_root = dir.path().join("data");
    let bin_dir = data_root.join("pi/v0.12.11");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::copy(&fake_pi, bin_dir.join("pi")).unwrap();

    let config = Config {
        port: 0,
        host: "127.0.0.1".to_string(),
        data_root,
        config_root: dir.path().join("config"),
        global_skills: dir.path().join("home/.agents/skills"),
        upstream: None,
        allow_remote: false,
        idle_timeout: emberloop_daemon::idle::DEFAULT_IDLE_TIMEOUT,
    };
    let daemon = Daemon::open(config).await.unwrap();
    let app = daemon.router();

    let (status, _) = request(
        &app,
        Method::PUT,
        "/api/providers/opencode-go/key",
        Some(json!({"key": "sk-test"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, models) = request(&app, Method::POST, "/api/models/refresh", None).await;
    assert_eq!(status, StatusCode::OK);
    let entries = models.as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|m| m["id"] == "opencode-go/glm-5" && m["label"] == "GLM-5"),
        "{models}"
    );

    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let (_, project) = request(
        &app,
        Method::POST,
        "/api/projects",
        Some(json!({"name": "demo", "root": root.display().to_string()})),
    )
    .await;
    let project_id = project["id"].as_str().unwrap();
    let (status, session) = request(
        &app,
        Method::POST,
        "/api/sessions",
        Some(json!({"projectId": project_id, "title": "pi", "model": "opencode-go/glm-5"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let session_id = session["id"].as_str().unwrap();

    let (status, body) = request(
        &app,
        Method::POST,
        &format!("/api/sessions/{session_id}/prompt"),
        Some(json!({"text": "hello pi"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn planned_update_resumes_active_agents_and_reviewers_but_not_idle_reviewers() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let env = harness(None).await;
    let (status, _) = request(
        &env.daemon.router(),
        Method::PUT,
        "/api/providers/opencode-go/key",
        Some(json!({"key": "sk-test"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let root = env._dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let (_, requester) = project_and_session(&env.daemon, &root).await;
    let template = env.daemon.sessions.get(&requester).await.unwrap();
    let mut active = Vec::new();
    for (id, role) in [
        ("ordinary", None),
        ("spec", Some("spec")),
        ("standards", Some("standards")),
        ("idle-reviewer", Some("spec")),
    ] {
        let mut meta = template.clone();
        meta.id = id.into();
        meta.model = "opencode-go/glm-5".into();
        meta.resume_token = Some(format!("saved-{id}"));
        meta.reviewer = role.map(|role| store::sessions::ReviewerAssignment {
            parent_id: if id == "idle-reviewer" {
                "ordinary".into()
            } else {
                requester.clone()
            },
            role: role.into(),
            ticket_path: "ticket.md".into(),
            starting_point: "abc123".into(),
        });
        std::fs::create_dir_all(env.daemon.data_root.session_dir(id)).unwrap();
        env.daemon.sessions.save(&meta).await.unwrap();
        if id != "idle-reviewer" {
            active.push(id.to_string());
        }
    }
    let mut meta = template;
    meta.model = "opencode-go/glm-5".into();
    env.daemon.sessions.save(&meta).await.unwrap();
    let config = env.daemon.config().clone();
    let bin_dir = config.data_root.join("pi/v0.12.11");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let binary = bin_dir.join("pi");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/^{"jsonrpc":"2.0","id":\([^,}]*\).*/\1/p')
  result='{"sessionId":"new-session","configOptions":[]}'
  case "$line" in
    *'"initialize"'*) result='{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}' ;;
    *'"session/prompt"'*)
      printf '%s\n' "$line" >> "$HOME/update-prompts.jsonl"
      result='{"stopReason":"end_turn"}' ;;
  esac
  printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    emberloop_daemon::update::write_pending(
        &config.data_root,
        &emberloop_daemon::update::PendingUpdate {
            from: "0.0.70".into(),
            to: "0.0.71".into(),
            session: Some(requester.clone()),
            also: active.clone(),
        },
    )
    .unwrap();
    let restarted = Daemon::open(config.clone()).await.unwrap();
    let server = tokio::spawn(restarted.serve());
    let received = |id: &str| {
        std::fs::read_to_string(
            config
                .data_root
                .join("pi-homes")
                .join(id)
                .join("update-prompts.jsonl"),
        )
        .unwrap_or_default()
    };
    let completed = tokio::time::timeout(Duration::from_secs(3), async {
        while received(&requester).is_empty() || active.iter().any(|id| received(id).is_empty()) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    server.abort();
    assert!(
        completed.is_ok(),
        "post-update continuation missing: {:?}",
        active
            .iter()
            .map(|id| (id, received(id)))
            .collect::<Vec<_>>()
    );
    for id in active {
        let text = received(&id);
        let requests: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(requests.len(), 1, "{id}: {text}");
        assert_eq!(requests[0]["params"]["sessionId"], format!("saved-{id}"));
        assert_eq!(requests[0]["params"]["prompt"][0]["text"], "continue");
    }
    assert!(received(&requester).contains("Continue where you left off."));
    assert!(received("idle-reviewer").is_empty());
}
