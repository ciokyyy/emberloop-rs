#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tempfile::TempDir;
use tools::{ToolConfig, Toolbox};

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    mcp: PathBuf,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("project");
    fs::create_dir(&root).expect("project root");
    let mcp = dir.path().join("mcp.json");
    Fixture {
        _dir: dir,
        root,
        mcp,
    }
}

/// A Toolbox whose `<config root>/mcp.json` holds `servers`.
fn toolbox_with(fixture: &Fixture, servers: Value) -> Arc<Toolbox> {
    fs::write(&fixture.mcp, json!({ "mcpServers": servers }).to_string()).expect("mcp config");
    let config = ToolConfig {
        mcp_file: Some(fixture.mcp.clone()),
        ..ToolConfig::new(&fixture.root, fixture.root.join("spill"))
    };
    Arc::new(Toolbox::new(config).expect("toolbox"))
}

fn fake(name: &str, extra: Value) -> Value {
    let mut entry = json!({ "command": env!("CARGO_BIN_EXE_fake-mcp") });
    let object = entry.as_object_mut().expect("object");
    for (key, value) in extra.as_object().expect("extra object") {
        object.insert(key.clone(), value.clone());
    }
    json!({ name: entry })
}

fn fake_tools(tools: &[(&str, &str)]) -> String {
    let list: Vec<Value> = tools
        .iter()
        .map(|(name, description)| {
            json!({"name": name, "description": description, "inputSchema": {"type": "object"}})
        })
        .collect();
    Value::Array(list).to_string()
}

#[tokio::test]
async fn figcraft_refuses_a_detached_document_without_a_live_server() {
    let fixture = setup();
    let toolbox = toolbox_with(&fixture, fake("figcraft", json!({})));
    let error = toolbox
        .call("figcraft.inspect_document", &json!({}))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("FigCraft live server"),
        "{error}"
    );
}

#[tokio::test]
async fn a_server_starts_in_the_project_root() {
    let fixture = setup();
    let toolbox = toolbox_with(
        &fixture,
        fake(
            "fake",
            json!({"env": {"FAKE_MCP_TOOLS": fake_tools(&[("pwd", "Where am I?")])}}),
        ),
    );

    let output = toolbox.call("fake.pwd", &json!({})).await.expect("pwd");
    let expected = fs::canonicalize(&fixture.root).expect("canonical root");
    assert_eq!(
        output,
        expected.to_string_lossy(),
        "auto-index needs this cwd"
    );
}

#[tokio::test]
async fn setup_commands_run_before_the_server_starts() {
    let fixture = setup();
    let log = fixture.root.join("setup.log");
    let toolbox = toolbox_with(
        &fixture,
        fake(
            "graph",
            json!({
                "env": {"FAKE_MCP_SETUP_LOG": log.to_string_lossy()},
                "setup": [["config", "set", "auto_index", "true"],
                          ["config", "set", "ignore_worktrees", "true"]],
            }),
        ),
    );

    toolbox
        .call("graph.echo", &json!({"x": 1}))
        .await
        .expect("call after setup");
    let written = fs::read_to_string(&log).expect("setup log");
    assert_eq!(
        written, "set auto_index true\nset ignore_worktrees true\n",
        "each setup argv tail reaches the binary in order"
    );
}

#[tokio::test]
async fn mcp_config_exposes_included_tools_only() {
    let fixture = setup();
    let toolbox = toolbox_with(
        &fixture,
        fake(
            "fake",
            json!({
                "include": ["echo"],
                "env": {"FAKE_MCP_TOOLS": fake_tools(&[("echo", "Echo."), ("secret", "Hide me.")])},
            }),
        ),
    );

    let tools = toolbox.mcp_tools().await.expect("tools");
    let names: Vec<&str> = tools.iter().map(|tool| tool.call_name.as_str()).collect();
    assert_eq!(names, vec!["fake.echo"], "got {names:?}");
    assert_eq!(tools[0].description, "Echo.");
}

#[tokio::test]
async fn mcp_call_reaches_the_server_and_returns_its_text() {
    let fixture = setup();
    let toolbox = toolbox_with(&fixture, fake("fake", json!({})));

    let out = toolbox
        .call("fake.echo", &json!({"x": 1}))
        .await
        .expect("call");
    assert_eq!(out, "echo:{\"x\":1}");

    let failed = toolbox.call("fake.broken", &json!({})).await;
    assert!(
        failed.unwrap_err().to_string().contains("boom"),
        "a tool error carries the server text"
    );
}

#[tokio::test]
async fn url_servers_are_refused_and_unknown_servers_stay_unknown() {
    let fixture = setup();
    let toolbox = toolbox_with(
        &fixture,
        json!({ "web": { "url": "https://example.com/mcp" } }),
    );

    let err = toolbox
        .call("web.search", &json!({}))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("stdio"), "got {err}");
    assert!(toolbox.mcp_tools().await.expect("tools").is_empty());

    let unknown = toolbox.call("nosuch.tool", &json!({})).await.unwrap_err();
    assert!(
        unknown.to_string().contains("unknown tool"),
        "got {unknown}"
    );
}

#[tokio::test]
async fn graph_calls_get_the_project_cbm_reports_for_this_root() {
    let fixture = setup();
    let projects = json!([
        {"name": "somewhere-else", "root_path": "/nope"},
        {"name": "the-project", "root_path": fixture.root.to_string_lossy()},
    ])
    .to_string();
    let toolbox = toolbox_with(
        &fixture,
        fake(
            "graph",
            json!({
                "include": ["search_graph"],
                "env": {
                    "FAKE_MCP_TOOLS": fake_tools(&[("search_graph", "Search the graph.")]),
                    "FAKE_MCP_PROJECTS": projects,
                },
            }),
        ),
    );

    let out = toolbox
        .call("graph.search_graph", &json!({"query": "auth"}))
        .await
        .expect("call");
    let arguments = out.strip_prefix("search_graph:").expect("tool name");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(arguments).expect("arguments"),
        json!({"project": "the-project", "query": "auth"})
    );
}

#[tokio::test]
async fn an_unindexed_root_falls_back_to_the_name_cbm_derives() {
    let fixture = setup();
    let expected = tools::graph::project_from_path(&fixture.root);
    let toolbox = toolbox_with(
        &fixture,
        fake(
            "graph",
            json!({
                "include": ["search_graph"],
                "env": {"FAKE_MCP_TOOLS": fake_tools(&[("search_graph", "Search the graph.")])},
            }),
        ),
    );

    let out = toolbox
        .call("graph.search_graph", &json!({"query": "auth"}))
        .await
        .expect("call");
    assert!(
        out.contains(&format!("\"project\":\"{expected}\"")),
        "{out}"
    );
}

#[tokio::test]
async fn execute_sees_mcp_tools_and_can_call_them() {
    let fixture = setup();
    let toolbox = toolbox_with(
        &fixture,
        fake(
            "graph",
            json!({
                "summary": "Code graph.",
                "include": ["search_graph", "trace_path"],
                "env": {
                    "FAKE_MCP_TOOLS": fake_tools(&[("search_graph", "Search the graph."),
                                                   ("trace_path", "Trace a path."),
                                                   ("detect_changes", "Hidden.")]),
                    "FAKE_MCP_PROJECTS": json!([{"name": "demo", "root_path": fixture.root.to_string_lossy()}]).to_string(),
                },
            }),
        ),
    );

    let out = toolbox
        .call(
            "execute",
            &json!({
                "code": "import tools\nprint(tools.servers())\nprint(tools.search_graph(query='auth'))\nprint(tools.search('graph'))"
            }),
        )
        .await
        .expect("execute");
    assert!(out.contains("graph"), "servers() lists graph: {out}");
    assert!(
        out.contains("\"project\":\"demo\""),
        "the project reaches the call: {out}"
    );
    assert!(out.contains("graph.search_graph"), "search finds it: {out}");
    assert!(!out.contains("detect_changes"), "excluded: {out}");
}

#[tokio::test]
async fn execute_does_not_wait_for_a_slow_server_to_start() {
    let fixture = setup();
    let toolbox = toolbox_with(
        &fixture,
        fake("slow", json!({"env": {"FAKE_MCP_DELAY_MS": "2500"}})),
    );

    let started = std::time::Instant::now();
    let out = toolbox
        .call("execute", &json!({"code": "print(1 + 1)"}))
        .await
        .expect("execute");
    assert!(out.contains('2'), "script ran: {out}");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(2400),
        "execute waited {:?} for the server",
        started.elapsed()
    );
    toolbox
        .call("slow.echo", &json!({}))
        .await
        .expect("the server keeps starting in the background");
}

#[tokio::test]
async fn a_broken_config_costs_the_mcp_tools_and_nothing_else() {
    let fixture = setup();
    fs::write(&fixture.mcp, "{not json").expect("mcp config");
    let config = ToolConfig {
        mcp_file: Some(fixture.mcp.clone()),
        ..ToolConfig::new(&fixture.root, fixture.root.join("spill"))
    };
    let toolbox = Arc::new(Toolbox::new(config).expect("toolbox"));

    assert!(toolbox.mcp_tools().await.is_err(), "the error surfaces");
    let err = toolbox
        .call("graph.search_graph", &json!({}))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("mcp.json"), "got {err}");
    assert!(
        toolbox
            .call("todo", &json!({"action": "read"}))
            .await
            .is_ok(),
        "builtin tools still work"
    );
}

#[test]
fn project_names_follow_cbm() {
    assert_eq!(
        tools::graph::project_from_path(Path::new("/tmp/cbm-spike/tiny")),
        "tmp-cbm-spike-tiny"
    );
}
