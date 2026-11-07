// temporary: drives the registry against the real codebase-memory-mcp
// A live spike against the real server: panics are the report, so the workspace
// no-unwrap lints are off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::path::PathBuf;

use tools::{ToolConfig, ToolConfig as _Cfg, Toolbox};

#[tokio::main]
async fn main() {
    let base = PathBuf::from("/tmp/cbm-live");
    let root = base.join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "fn handler(x: u32) -> u32 { helper(x) + 1 }\nfn helper(y: u32) -> u32 { y * 2 }\nfn main() { println!(\"{}\", handler(3)); }\n",
    )
    .unwrap();

    let mcp = base.join("mcp.json");
    std::fs::write(
        &mcp,
        serde_json::json!({"mcpServers":{"graph":{
            "command":"npx",
            "args":["-y","codebase-memory-mcp@0.12.1"],
            "env":{"CBM_CACHE_DIR": base.join("cache").to_string_lossy()},
            "include":["search_graph","trace_path","get_code_snippet","get_file_outline","get_architecture"],
            "summary":"code graph",
            "setup":[["config","set","auto_index","true"],["config","set","auto_watch","true"],["config","set","ignore_worktrees","true"]]
        }}})
        .to_string(),
    )
    .unwrap();

    let toolbox = Toolbox::new(ToolConfig {
        mcp_file: Some(mcp),
        .._Cfg::new(&root, base.join("spill"))
    })
    .expect("toolbox");

    match toolbox.mcp_tools().await {
        Ok(tools) => {
            println!(
                "TOOLS={:?}",
                tools.iter().map(|t| &t.call_name).collect::<Vec<_>>()
            );
        }
        Err(err) => println!("TOOLS_ERR={err}"),
    }

    let args = serde_json::json!({"query": "handler"});
    match toolbox.call("graph.search_graph", &args).await {
        Ok(out) => println!("CALL_OK={}", &out[..out.len().min(600)]),
        Err(err) => println!("CALL_ERR={err}"),
    }
}
