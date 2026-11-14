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
    for name in ["bash", "write", "read", "execute", "submit"] {
        assert!(names.contains(&name.into()), "{name}");
    }
    for name in ["spawn", "steer", "stop", "result"] {
        assert!(
            !names.contains(&name.into()),
            "subagent should not see {name}"
        );
    }
    assert!(toolbox
        .call("edit", &json!({"input":""}))
        .await
        .unwrap_err()
        .to_string()
        .contains("reviewer"));
    assert!(toolbox
        .call("spawn", &json!({"role":"worker","text":"nested"}))
        .await
        .unwrap_err()
        .to_string()
        .contains("subagent"));
    let result = toolbox.call("execute", &json!({"code":"import tools\ntry:\n tools.call('edit', input='')\nexcept tools.ToolError as error:\n print(error)"})).await.unwrap();
    assert!(result.contains("edit is unavailable to reviewer Sessions"));
}

#[tokio::test]
async fn workers_keep_edit_and_write_and_lose_subagent_management_tools() {
    let dir = tempfile::tempdir().unwrap();
    let toolbox = Toolbox::new(ToolConfig {
        session_url: Some("http://127.0.0.1:1/api/sessions/worker".into()),
        subagent_role: Some("worker".into()),
        ..ToolConfig::new(dir.path(), dir.path().join("spill"))
    })
    .unwrap();
    let server = McpServer::new(Arc::new(toolbox.clone()));
    let names: Vec<_> = server
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    for name in ["edit", "write", "bash", "read", "execute", "submit"] {
        assert!(names.contains(&name.into()), "{name}");
    }
    for name in ["spawn", "steer", "stop", "result"] {
        assert!(
            !names.contains(&name.into()),
            "worker should not see {name}"
        );
    }
    assert!(toolbox
        .call("steer", &json!({"id":"x","text":"nested"}))
        .await
        .unwrap_err()
        .to_string()
        .contains("subagent"));
}

#[tokio::test]
async fn scouts_lose_edit_and_write_and_enforce_readonly_bash_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "hello scout\n").unwrap();
    let toolbox = Toolbox::new(ToolConfig {
        session_url: Some("http://127.0.0.1:1/api/sessions/scout".into()),
        subagent_role: Some("scout".into()),
        ..ToolConfig::new(dir.path(), dir.path().join("spill"))
    })
    .unwrap();
    let server = McpServer::new(Arc::new(toolbox.clone()));
    let names: Vec<_> = server
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    for forbidden in ["edit", "write", "spawn", "steer", "stop", "result"] {
        assert!(
            !names.contains(&forbidden.into()),
            "scout must omit {forbidden}"
        );
    }
    for allowed in ["bash", "read", "grep", "find", "execute", "submit"] {
        assert!(names.contains(&allowed.into()), "scout must keep {allowed}");
    }

    // Direct and execute-bridged edit/write fail.
    for blocked_tool in [
        ("edit", json!({"input":""})),
        ("write", json!({"path":"x.txt","content":"no"})),
    ] {
        assert!(toolbox
            .call(blocked_tool.0, &blocked_tool.1)
            .await
            .unwrap_err()
            .to_string()
            .contains("unavailable to scout"));
    }
    let exec_write = toolbox
        .call(
            "execute",
            &json!({"code":"import tools\nfor fn in (lambda: tools.edit(input=''), lambda: tools.write(path='x', content='y')):\n try:\n  fn()\n except tools.ToolError as e:\n  print(e)"}),
        )
        .await
        .unwrap();
    assert!(exec_write.contains("edit is unavailable to scout Sessions"));
    assert!(exec_write.contains("write is unavailable to scout Sessions"));

    // Read-only commands succeed directly and via execute.
    for ok_cmd in [
        "ls -la",
        "cat notes.txt",
        "grep hello notes.txt | wc -l",
        "pwd && head -n 1 notes.txt",
        "find . -maxdepth 1",
    ] {
        assert!(
            toolbox
                .call("bash", &json!({"command": ok_cmd}))
                .await
                .is_ok(),
            "expected scout bash to allow: {ok_cmd}"
        );
    }
    let exec_bash = toolbox
        .call(
            "execute",
            &json!({"code":"import tools\nprint(tools.bash(command='cat notes.txt'))\ntry:\n tools.bash(command='rm notes.txt')\nexcept tools.ToolError as e:\n print('BLOCKED:', e)"}),
        )
        .await
        .unwrap();
    assert!(exec_bash.contains("hello scout"));
    assert!(exec_bash.contains("BLOCKED:"));

    // Mutating commands, redirects, and arbitrary shell execution fail closed.
    for bad_cmd in [
        "rm notes.txt",
        "touch new.txt",
        "mkdir sub",
        "echo hi > out.txt",
        "cat notes.txt >> out.txt",
        "find . -delete",
        "find . -exec rm {} +",
        "fd -x rm",
        "rg --pre=sh hello",
        "rg --hostname-bin=sh hello",
        "tree -o out.txt",
        "tree --output=out.txt",
        "file -C",
        "file --compile",
        "git commit -m nope",
        "git branch new-branch",
        "git -c core.fsmonitor=evil status",
        "git grep -Osh pattern",
        "git grep --open-files-in-pager=sh pattern",
        "git diff --no-index a b",
        "git log -p --textconv",
        "cargo test",
        "cargo --config foo=bar check",
        "python3 -c 'print(1)'",
        "sh -c ls",
        "ls $(id)",
        "ls `id`",
        "ls && rm notes.txt",
        "ls2>&1",
        "2>&1cat",
        "rtk rm notes.txt",
        "rtk rtk ls",
        "PATH=/tmp ls",
        "FOO=1 ls",
    ] {
        assert!(
            toolbox
                .call("bash", &json!({"command": bad_cmd}))
                .await
                .is_err(),
            "expected scout bash to reject: {bad_cmd}"
        );
    }

    // Allowed git/cargo/rtk and quoted/glob forms pass validation.
    assert!(
        tools::intercept::scout_blocked("git status --short && git log -n 1 | head -n 1").is_none()
    );
    assert!(tools::intercept::scout_blocked(
        "git branch --list && cargo metadata --no-deps && rtk ls"
    )
    .is_none());
    assert!(
        tools::intercept::scout_blocked("cargo check -p tools && cargo tree -p tools").is_none()
    );
    assert!(
        tools::intercept::scout_blocked("grep \"a;b\" notes.txt && ls *.txt 2>&1 && ls $PWD")
            .is_none()
    );
    assert!(tools::intercept::scout_blocked("tree && fd Cargo.toml && jq . package.json && file notes.txt && stat notes.txt && which ls").is_none());
}
