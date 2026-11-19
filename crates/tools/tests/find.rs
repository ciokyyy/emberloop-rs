#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `find` sends `architecture` and unpathed `trace` through the graph MCP server and
//! falls back locally, without failing the call, whenever the graph is unusable.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tempfile::TempDir;
use tools::{ToolConfig, Toolbox};

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    mcp: PathBuf,
    calls: PathBuf,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/widget.rs"),
        "pub fn frobulate_widget() -> i32 {\n    1\n}\n",
    )
    .unwrap();
    let mcp = dir.path().join("mcp.json");
    let calls = dir.path().join("calls.log");
    Fixture {
        _dir: dir,
        root,
        mcp,
        calls,
    }
}

/// A `graph` server whose calls are logged, so a test can see whether `find` used it.
fn configured(fixture: &Fixture, mode: Option<&str>) -> Value {
    let mut env = json!({"FAKE_MCP_CALL_LOG": fixture.calls.to_string_lossy()});
    if let Some(mode) = mode {
        env["FAKE_MCP_GRAPH_MODE"] = json!(mode);
    }
    json!({
        "graph": {
            "command": env!("CARGO_BIN_EXE_fake-mcp"),
            "env": env,
        }
    })
}

fn toolbox(
    fixture: &Fixture,
    servers: Value,
    max_call_secs: Option<u64>,
    role: Option<&str>,
) -> Arc<Toolbox> {
    fs::write(&fixture.mcp, json!({ "mcpServers": servers }).to_string()).unwrap();
    let config = ToolConfig {
        mcp_file: Some(fixture.mcp.clone()),
        max_call_secs,
        subagent_role: role.map(str::to_string),
        ..ToolConfig::new(&fixture.root, fixture.root.join("spill"))
    };
    Arc::new(Toolbox::new(config).unwrap())
}

fn calls(fixture: &Fixture) -> String {
    fs::read_to_string(&fixture.calls).unwrap_or_default()
}

fn reset_calls(fixture: &Fixture) {
    fs::write(&fixture.calls, "").unwrap();
}

fn seen_state(fixture: &Fixture) -> String {
    fs::read_to_string(fixture.root.join("seen/state.json")).unwrap_or_default()
}

async fn find(toolbox: &Toolbox, args: Value) -> String {
    toolbox.call("find", &args).await.expect("find")
}

#[tokio::test]
async fn architecture_uses_the_graph_while_outline_and_search_stay_local() {
    let fixture = setup();
    let toolbox = toolbox(&fixture, configured(&fixture, None), None, None);

    let out = find(&toolbox, json!({"mode": "architecture"})).await;
    assert!(out.starts_with("[graph]"), "{out}");
    assert!(calls(&fixture).contains("get_architecture"));
    assert_eq!(
        seen_state(&fixture),
        "",
        "the graph path recorded seen lines"
    );

    reset_calls(&fixture);
    let out = find(
        &toolbox,
        json!({"mode": "outline", "path": "src/widget.rs"}),
    )
    .await;
    assert!(out.contains("frobulate_widget"), "{out}");
    assert_eq!(calls(&fixture), "", "outline reached the graph");
    assert_eq!(seen_state(&fixture), "", "outline recorded seen lines");

    let out = find(
        &toolbox,
        json!({"query": "frobulate_widget", "grep_keywords": ["frobulate_widget"]}),
    )
    .await;
    assert!(out.contains("[grep]"), "{out}");
    assert_eq!(calls(&fixture), "", "search reached the graph");
    assert!(
        seen_state(&fixture).contains("widget.rs"),
        "search must keep recording seen lines"
    );
}

#[tokio::test]
async fn trace_sends_direction_and_depth_and_a_pathed_trace_stays_local() {
    let fixture = setup();
    let toolbox = toolbox(&fixture, configured(&fixture, None), None, None);

    let out = find(
        &toolbox,
        json!({"mode": "trace", "query": "deliver_idle", "direction": "outbound", "depth": 3}),
    )
    .await;
    assert!(out.starts_with("[graph]"), "{out}");
    let log = calls(&fixture);
    assert!(log.contains("trace_path"), "{log}");
    assert!(log.contains("\"direction\":\"outbound\""), "{log}");
    assert!(log.contains("\"depth\":3"), "{log}");

    reset_calls(&fixture);
    find(&toolbox, json!({"mode": "trace", "query": "deliver_idle"})).await;
    let log = calls(&fixture);
    assert!(log.contains("\"direction\":\"inbound\""), "{log}");
    assert!(log.contains("\"depth\":2"), "{log}");

    reset_calls(&fixture);
    find(
        &toolbox,
        json!({"mode": "trace", "query": "deliver_idle", "depth": 99}),
    )
    .await;
    assert!(
        calls(&fixture).contains("\"depth\":5"),
        "depth must clamp to 5: {}",
        calls(&fixture)
    );

    reset_calls(&fixture);
    let out = find(
        &toolbox,
        json!({"mode": "trace", "query": "frobulate_widget", "path": "src"}),
    )
    .await;
    assert!(out.contains("[grep]"), "{out}");
    assert_eq!(calls(&fixture), "", "a pathed trace reached the graph");
}

#[tokio::test]
async fn a_slow_cold_connect_falls_back_then_warms_without_poisoning_the_cooldown() {
    let fixture = setup();
    let mut servers = configured(&fixture, None);
    servers["graph"]["env"]["FAKE_MCP_DELAY_MS"] = json!("3000");
    let toolbox = toolbox(&fixture, servers, Some(2), None);

    let first = find(&toolbox, json!({"mode": "architecture"})).await;
    assert!(first.starts_with("architecture summary"), "{first}");
    let second = find(&toolbox, json!({"mode": "architecture"})).await;
    assert!(second.starts_with("[graph]"), "{second}");
    assert!(
        calls(&fixture).contains("get_architecture"),
        "the warm connect was never used"
    );
}

#[tokio::test]
async fn a_post_connect_rpc_timeout_cools_the_graph() {
    let fixture = setup();
    let mut servers = configured(&fixture, None);
    servers["graph"]["env"]["FAKE_MCP_GRAPH_DELAY_MS"] = json!("1500");
    let toolbox = toolbox(&fixture, servers, Some(1), None);

    for _ in 0..2 {
        let out = find(&toolbox, json!({"mode": "architecture"})).await;
        assert!(out.starts_with("architecture summary"), "{out}");
    }
    let log = calls(&fixture);
    assert_eq!(
        log.matches("get_architecture").count(),
        1,
        "the cooled graph was called again: {log}"
    );
}

#[tokio::test]
async fn a_connect_failure_falls_back_and_cools_the_graph() {
    let fixture = setup();
    let starts = fixture.root.join("starts.log");
    let mut servers = configured(&fixture, None);
    servers["graph"]["env"]["FAKE_MCP_EXIT"] = json!("1");
    servers["graph"]["env"]["FAKE_MCP_START_LOG"] = json!(starts);
    let toolbox = toolbox(&fixture, servers, Some(1), None);

    for _ in 0..2 {
        let out = find(&toolbox, json!({"mode": "architecture"})).await;
        assert!(out.starts_with("architecture summary"), "{out}");
    }
    assert_eq!(calls(&fixture), "", "a dead server answered a call");
    let starts = fs::read_to_string(starts).unwrap_or_default();
    assert_eq!(
        starts.lines().count(),
        1,
        "the cooled graph was started again: {starts}"
    );
}

#[tokio::test]
async fn a_graph_rpc_failure_cools_the_graph_for_sixty_seconds() {
    let fixture = setup();
    let toolbox = toolbox(&fixture, configured(&fixture, Some("error")), None, None);

    for _ in 0..2 {
        let out = find(&toolbox, json!({"mode": "architecture"})).await;
        assert!(out.starts_with("architecture summary"), "{out}");
    }
    let log = calls(&fixture);
    assert_eq!(
        log.matches("get_architecture").count(),
        1,
        "the cooled graph was called again: {log}"
    );
}

#[tokio::test]
async fn truncated_and_empty_graph_answers_fall_back_without_cooling() {
    for mode in ["truncated", "empty"] {
        let fixture = setup();
        let toolbox = toolbox(&fixture, configured(&fixture, Some(mode)), None, None);
        for _ in 0..2 {
            let out = find(&toolbox, json!({"mode": "architecture"})).await;
            assert!(out.starts_with("architecture summary"), "{mode}: {out}");
        }
        assert_eq!(
            calls(&fixture).matches("get_architecture").count(),
            2,
            "{mode}: a bad answer must not cool the graph"
        );
    }
}

#[tokio::test]
async fn an_unconfigured_graph_or_a_goldfish_never_connects() {
    let fixture = setup();
    let unconfigured = Arc::new(
        Toolbox::new(ToolConfig::new(
            &fixture.root,
            fixture.root.join("spill-unconfigured"),
        ))
        .unwrap(),
    );
    let out = find(&unconfigured, json!({"mode": "architecture"})).await;
    assert!(out.starts_with("architecture summary"), "{out}");

    let goldfish = toolbox(&fixture, configured(&fixture, None), None, Some("goldfish"));
    let out = find(&goldfish, json!({"mode": "architecture"})).await;
    assert!(out.starts_with("architecture summary"), "{out}");
    assert_eq!(calls(&fixture), "", "a goldfish reached the graph");
}
