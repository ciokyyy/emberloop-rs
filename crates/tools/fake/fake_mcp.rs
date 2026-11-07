//! A scripted stdio MCP server for the client tests: answers the handshake,
//! lists whatever `FAKE_MCP_TOOLS` names, and echoes each tool call's
//! arguments back as text, so a test can assert what the client sent.
use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn default_tools() -> Value {
    json!([
        {"name": "echo", "description": "Echo the arguments.", "inputSchema": {"type": "object"}},
        {"name": "broken", "description": "Answer isError.", "inputSchema": {"type": "object"}},
    ])
}

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("config") {
        // A setup command: append its argv tail to FAKE_MCP_SETUP_LOG and leave.
        let rest: Vec<String> = args.collect();
        if let Some(log) = std::env::var_os("FAKE_MCP_SETUP_LOG") {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
            {
                let _ = writeln!(file, "{}", rest.join(" "));
            }
        }
        return;
    }
    if std::env::var_os("FAKE_MCP_EXIT").is_some() {
        std::process::exit(1);
    }
    let tools = std::env::var("FAKE_MCP_TOOLS")
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or_else(default_tools);
    let projects = std::env::var("FAKE_MCP_PROJECTS").unwrap_or_else(|_| "[]".to_string());
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = request["method"].as_str().unwrap_or_default();
        let Some(id) = request.get("id").cloned() else {
            continue; // A notification is never answered.
        };
        let params = &request["params"];
        let reply = match method {
            "initialize" => json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake", "version": "1"},
            }),
            "tools/list" => json!({ "tools": tools }),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or_default();
                if name == "broken" {
                    json!({"content": [{"type": "text", "text": "boom"}], "isError": true})
                } else if name == "list_projects" {
                    let text = format!("{{\"projects\":{projects}}}");
                    json!({"content": [{"type": "text", "text": text}]})
                } else if name == "pwd" {
                    let cwd = std::env::current_dir().unwrap_or_default();
                    json!({"content": [{"type": "text", "text": cwd.to_string_lossy()}]})
                } else {
                    let args = params.get("arguments").cloned().unwrap_or(json!({}));
                    json!({"content": [{"type": "text", "text": format!("{name}:{args}")}]})
                }
            }
            _ => json!({}),
        };
        let frame = json!({"jsonrpc": "2.0", "id": id, "result": reply});
        if writeln!(out, "{frame}").is_err() || out.flush().is_err() {
            break;
        }
    }
}
