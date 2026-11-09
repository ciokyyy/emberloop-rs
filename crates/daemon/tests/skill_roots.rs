#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;

use emberloop_daemon::mcp_tool_config;
use serde_json::json;
use tools::Toolbox;

#[tokio::test]
async fn mcp_reads_global_skills_only_when_given_the_global_root() {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let project = base.join("project");
    let global = base.join("home/.agents/skills");
    let skill = global.join("foo/SKILL.md");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&skill, "---\nname: foo\n---\nbody\n").unwrap();
    let config_root = base.join("config");
    let args = json!({ "path": skill.display().to_string() });

    let with_root = Toolbox::new(mcp_tool_config(
        project.clone(),
        base.join("spill"),
        &config_root,
        Some(global.clone()),
    ))
    .unwrap();
    let read = with_root.call("read", &args).await.unwrap();
    assert!(read.contains("name: foo"), "{read}");

    let without_root = Toolbox::new(mcp_tool_config(
        project,
        base.join("spill"),
        &config_root,
        None,
    ))
    .unwrap();
    assert!(without_root.call("read", &args).await.is_err());
}
