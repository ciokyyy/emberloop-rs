#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::io::Write;
use std::process::{Command, Stdio};

// The MCP subprocess inherits RUST_LOG; logs on stdout break JSON-RPC framing.
#[test]
fn mcp_stdout_carries_only_json_rpc_even_with_rust_log() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_emberloop-daemon"))
        .args(["mcp", "--root"])
        .arg(dir.path())
        .arg("--spill")
        .arg(dir.path().join("spill"))
        .env("RUST_LOG", "info")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
    writeln!(child.stdin.take().unwrap(), "{init}").unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.is_empty());
    for line in text.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|_| panic!("{line:?}"));
    }
}
