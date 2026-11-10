#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::sync::Arc;
use tools::{McpServer, ToolConfig, Toolbox};

#[tokio::test]
async fn reviewers_lose_only_edit_and_cannot_reach_it_through_execute() {
    let dir = tempfile::tempdir().unwrap();
    let toolbox = Toolbox::new(ToolConfig {
        session_url: Some("http://127.0.0.1:1/api/sessions/reviewer".into()),
        reviewer: true,
        ..ToolConfig::new(dir.path(), dir.path().join("spill"))
    })
    .unwrap();
    let server = McpServer::new(Arc::new(toolbox.clone()));
    let names: Vec<_> = server
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(!names.contains(&"edit".into()));
    for name in [
        "bash", "write", "read", "execute", "submit", "spawn", "steer", "stop", "result",
    ] {
        assert!(names.contains(&name.into()), "{name}");
    }
    assert!(toolbox
        .call("edit", &json!({"input":""}))
        .await
        .unwrap_err()
        .to_string()
        .contains("reviewer"));
    let result = toolbox.call("execute", &json!({"code":"import tools\ntry:\n tools.call('edit', input='')\nexcept tools.ToolError as error:\n print(error)"})).await.unwrap();
    assert!(result.contains("edit is unavailable to reviewer Sessions"));
}
