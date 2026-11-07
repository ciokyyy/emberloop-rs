//! The `graph` MCP server entry: codebase-memory-mcp, reachable from `execute`.
//!
//! The entry lives in `<config root>/mcp.json` next to any server a user adds
//! by hand. An existing `graph` entry always wins, so a user override survives
//! every start. Nothing is spawned here; the tools crate connects lazily.

use std::path::Path;

use serde_json::{json, Map, Value};

/// The codebase-memory-mcp version the `npx` fallback pins.
const VERSION: &str = "0.12.1";

/// The exploration tools the skill teaches. Everything else stays hidden.
pub const TOOLS: [&str; 5] = [
    "search_graph",
    "trace_path",
    "get_code_snippet",
    "get_file_outline",
    "get_architecture",
];

const SUMMARY: &str = "Static code graph of this project: symbols, call paths, snippets, outlines and architecture. `project` is filled in for you.";

/// Write the `graph` entry into `<config root>/mcp.json` when it has none, and
/// report whether the graph is usable. Unparseable config is left alone.
pub fn ensure(config_root: &Path) -> bool {
    let file = config_root.join("mcp.json");
    let Some(mut config) = read(&file) else {
        return false;
    };
    match insert(&mut config, config_root) {
        None => false,
        Some(false) => enabled(&config),
        Some(true) => {
            if let Err(error) = write(&file, &config) {
                tracing::warn!(%error, path = %file.display(), "cannot write mcp.json");
                return false;
            }
            enabled(&config)
        }
    }
}

/// The parsed file, an empty object when it does not exist, and `None` when it
/// exists but is not JSON: a hand-broken file is never overwritten.
fn read(file: &Path) -> Option<Value> {
    match std::fs::read_to_string(file) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(config) => Some(config),
            Err(error) => {
                tracing::warn!(%error, path = %file.display(), "mcp.json is not JSON; leaving it alone");
                None
            }
        },
        Err(_) => Some(json!({})),
    }
}

/// Insert the entry when absent; `None` when `mcpServers` is not an object.
fn insert(config: &mut Value, config_root: &Path) -> Option<bool> {
    let servers = config.as_object_mut().and_then(|root| {
        root.entry("mcpServers")
            .or_insert_with(|| json!({}))
            .as_object_mut()
    });
    let Some(servers) = servers else {
        tracing::warn!("mcpServers in mcp.json is not an object; leaving it alone");
        return None;
    };
    if servers.contains_key("graph") {
        return Some(false);
    }
    servers.insert("graph".to_string(), entry(config_root));
    Some(true)
}

fn enabled(config: &Value) -> bool {
    config
        .pointer("/mcpServers/graph/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn entry(config_root: &Path) -> Value {
    let cache = config_root.join("code-index");
    let (command, args) = if on_path("codebase-memory-mcp") {
        ("codebase-memory-mcp", json!([]))
    } else {
        (
            "npx",
            json!(["-y", format!("codebase-memory-mcp@{VERSION}")]),
        )
    };
    let mut env = Map::new();
    env.insert(
        "CBM_CACHE_DIR".to_string(),
        Value::String(cache.to_string_lossy().into_owned()),
    );
    json!({
        "command": command,
        "args": args,
        "env": env,
        "include": TOOLS,
        "summary": SUMMARY,
        "setup": [
            ["config", "set", "auto_index", "true"],
            ["config", "set", "auto_watch", "true"],
            ["config", "set", "ignore_worktrees", "true"],
        ],
    })
}

fn write(file: &Path, config: &Value) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(config)?;
    text.push('\n');
    std::fs::write(file, text)
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn read(root: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(root.join("mcp.json")).unwrap()).unwrap()
    }

    #[test]
    fn creates_the_file_with_the_graph_entry() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ensure(dir.path()));
        let graph = &read(dir.path())["mcpServers"]["graph"];
        assert_eq!(graph["include"], json!(TOOLS));
        assert_eq!(
            graph["setup"][0],
            json!(["config", "set", "auto_index", "true"])
        );
        assert_eq!(
            graph["env"]["CBM_CACHE_DIR"],
            json!(dir.path().join("code-index").to_string_lossy())
        );
        let command = graph["command"].as_str().unwrap();
        assert!(command == "codebase-memory-mcp" || command == "npx");
    }

    #[test]
    fn keeps_an_existing_entry_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let mine = json!({"mcpServers": {"graph": {"command": "mine", "include": ["x"]}, "other": {"command": "y"}}});
        std::fs::write(dir.path().join("mcp.json"), mine.to_string()).unwrap();
        assert!(ensure(dir.path()));
        assert_eq!(read(dir.path()), mine);
    }

    #[test]
    fn a_disabled_entry_is_not_a_graph() {
        let dir = tempfile::tempdir().unwrap();
        let mine = json!({"mcpServers": {"graph": {"command": "mine", "enabled": false}}});
        std::fs::write(dir.path().join("mcp.json"), mine.to_string()).unwrap();
        assert!(!ensure(dir.path()));
        assert_eq!(read(dir.path()), mine);
    }

    #[test]
    fn broken_json_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mcp.json"), "{not json").unwrap();
        assert!(!ensure(dir.path()));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("mcp.json")).unwrap(),
            "{not json"
        );
    }
}
