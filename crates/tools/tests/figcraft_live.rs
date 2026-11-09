#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Explicit native integration check; not part of the fast, hermetic repository suite.
use serde_json::json;
use std::path::Path;
use tools::mcpclient::McpRegistry;

async fn server(data: &Path) -> tokio::process::Child {
    let child = tokio::process::Command::new("figcraft-server")
        .args(["serve", "--listen", "127.0.0.1:47900", "--data"])
        .arg(data)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut readiness = tokio::time::interval(std::time::Duration::from_millis(25));
    for _ in 0..100 {
        if tokio::net::TcpStream::connect("127.0.0.1:47900")
            .await
            .is_ok()
        {
            return child;
        }
        readiness.tick().await;
    }
    panic!("FigCraft server failed to start");
}

async fn contains(client: &McpRegistry, name: &str) {
    let mut progress = tokio::time::interval(std::time::Duration::from_millis(25));
    for _ in 0..100 {
        let document = client
            .call("figcraft.inspect_document", &json!({}))
            .await
            .unwrap();
        if document.contains(name) {
            return;
        }
        progress.tick().await;
    }
    panic!("live edit did not reach the other client");
}

#[tokio::test]
#[ignore = "requires installed upstream FigCraft 0.16.0 binaries and exclusive port 47900"]
async fn clients_share_one_project_document_and_rejoin_after_server_restart() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("server");
    let mut child = server(&data).await;
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let config = dir.path().join("mcp.json");
    std::fs::write(
        &config,
        json!({"mcpServers":{"figcraft":{
            "command":"figcraft-cli", "args":["mcp"],
            "env":{"FIGCRAFT_LIVE_DIR":dir.path().join("links"),"FIGCRAFT_SERVER_DATA":data}
        }}})
        .to_string(),
    )
    .unwrap();
    let first = McpRegistry::load(&project, Some(&config));
    let link = first.figcraft_link().await.unwrap();
    let second = McpRegistry::load(&project, Some(&config));
    assert_eq!(second.figcraft_link().await.unwrap(), link);
    first
        .call(
            "figcraft.create_node",
            &json!({"type":"frame","name":"Shared upstream frame","width":120,"height":80}),
        )
        .await
        .unwrap();
    contains(&second, "Shared upstream frame").await;
    second
        .call(
            "figcraft.create_node",
            &json!({"type":"rectangle","name":"Second client edit","width":20,"height":20}),
        )
        .await
        .unwrap();
    contains(&first, "Second client edit").await;
    let other = dir.path().join("other-project");
    std::fs::create_dir(&other).unwrap();
    let isolated = McpRegistry::load(&other, Some(&config));
    assert_ne!(isolated.figcraft_link().await.unwrap(), link);
    assert!(!isolated
        .call("figcraft.inspect_document", &json!({}))
        .await
        .unwrap()
        .contains("Shared upstream frame"));
    drop((first, second, isolated));
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    let mut restarted = server(&data).await;
    let restored = McpRegistry::load(&project, Some(&config));
    assert_eq!(restored.figcraft_link().await.unwrap(), link);
    contains(&restored, "Shared upstream frame").await;
    contains(&restored, "Second client edit").await;
    restarted.kill().await.unwrap();
}
