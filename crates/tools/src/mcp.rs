use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Toolbox;

/// MCP server over rmcp: advertises the workspace tools and dispatches calls.
pub struct McpServer {
    toolbox: Arc<Toolbox>,
    descriptions: Arc<HashMap<String, String>>,
    capture_seq: Arc<AtomicU64>,
}

impl Clone for McpServer {
    fn clone(&self) -> Self {
        Self {
            toolbox: Arc::clone(&self.toolbox),
            descriptions: Arc::clone(&self.descriptions),
            capture_seq: Arc::clone(&self.capture_seq),
        }
    }
}

impl McpServer {
    /// A server that advertises the embedded Prompt Store texts.
    pub fn new(toolbox: Arc<Toolbox>) -> Self {
        Self {
            toolbox,
            descriptions: Arc::new(default_descriptions()),
            capture_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Tool descriptions from the daemon's Prompt Store (user overrides included), keyed by
    /// tool name: the web UI edits `tool.<name>.description` and this is where it lands.
    pub fn with_descriptions(mut self, descriptions: HashMap<String, String>) -> Self {
        self.descriptions = Arc::new(descriptions);
        self
    }

    pub fn tools(&self) -> Vec<Tool> {
        tool_specs()
            .into_iter()
            .filter_map(|spec| {
                let name = spec.get("name")?.as_str()?.to_string();
                // The todo tool is switched off for now; its code and tests stay.
                if name == "todo" || !self.toolbox.tool_available(&name) {
                    return None;
                }
                let description = self.descriptions.get(&name).cloned().unwrap_or_default();
                let schema: Map<String, Value> =
                    serde_json::from_value(spec.get("inputSchema")?.clone()).ok()?;
                Some(Tool::new(name, description, Arc::new(schema)))
            })
            .collect()
    }

    pub async fn call(&self, name: &str, arguments: Option<Map<String, Value>>) -> CallToolResult {
        let args = arguments.map_or_else(|| json!({}), Value::Object);
        if name == "read" {
            if let Some(found) = crate::read::image(&self.toolbox, &args) {
                return match found {
                    Ok((mime, data)) => {
                        self.record_captured_output(name, &args, &format!("[image {mime}]"));
                        CallToolResult::success(vec![ContentBlock::image(data, mime)])
                    }
                    Err(error) => {
                        let msg = error.to_string();
                        self.record_captured_output(name, &args, &msg);
                        CallToolResult::error(vec![ContentBlock::text(msg)])
                    }
                };
            }
        }
        match self.toolbox.call(name, &args).await {
            Ok(text) => {
                self.record_captured_output(name, &args, &text);
                CallToolResult::success(vec![ContentBlock::text(text)])
            }
            Err(error) => {
                let msg = error.to_string();
                self.record_captured_output(name, &args, &msg);
                CallToolResult::error(vec![ContentBlock::text(msg)])
            }
        }
    }

    fn record_captured_output(&self, name: &str, args: &Value, output: &str) {
        if !self.toolbox.capture_mcp_output() {
            return;
        }
        let dir = self.toolbox.spill_dir().join("mcp_outputs");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let seq = self.capture_seq.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let stem = format!("{millis:016x}_{pid:08x}_{seq:08x}");
        let tmp = dir.join(format!("{stem}.tmp"));
        let final_path = dir.join(format!("{stem}.json"));
        let capped = truncate_utf8(output, MAX_CAPTURED_BYTES);
        let payload = json!({
            "name": name,
            "args": strip_meta_keys(args),
            "output": capped,
        });
        if let Ok(bytes) = serde_json::to_vec(&payload) {
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &final_path);
            }
        }
    }

    pub async fn serve_stdio(
        self: Arc<Self>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let service = (*self).clone().serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("emberloop-tools", env!("CARGO_PKG_VERSION")),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        Ok(self.call(&request.name, request.arguments).await.into())
    }
}

/// Embedded Prompt Store texts for every tool, keyed by tool name.
fn default_descriptions() -> HashMap<String, String> {
    prompts::PROMPT_IDS
        .iter()
        .filter_map(|id| {
            let name = prompts::tool_name(id)?;
            Some((name.to_string(), prompts::default_text(id).to_string()))
        })
        .collect()
}

fn tool_specs() -> Vec<Value> {
    let specs = json!([
        {
            "name": "spawn",
            "inputSchema": {"type": "object", "properties": {
                "role": {"type": "string", "enum": ["spec", "standards", "worker", "scout"]},
                "tier": {"type": "string", "enum": ["smol", "mid", "top"]},
                "ticketPath": {"type": "string"}, "startingPoint": {"type": "string"},
                "text": {"type": "string"}, "title": {"type": "string"}
            }, "required": ["role"]}
        },
        {
            "name": "steer",
            "inputSchema": {"type": "object", "properties": {
                "id": {"type": "string"}, "text": {"type": "string"},
                "tier": {"type": "string", "enum": ["smol", "mid", "top"]},
                "ticketPath": {"type": "string"}, "startingPoint": {"type": "string"}
            }, "required": ["id", "text"]}
        },
        {
            "name": "stop",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}
        },
        {
            "name": "result",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}
        },
        {
            "name": "submit",
            "inputSchema": {"type": "object", "properties": {
                "status": {"type": "string", "enum": ["done", "blocked"]},
                "report": {"type": "string"}, "summary": {"type": "string"}
            }, "required": ["status"]}
        },
        {
            "name": "read",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File, directory, http(s) URL, ctx:<handle> pointer, el://skills/<name> or el://docs/<topic>. Append :raw to a file or URL path to read it whole; without it the first 300 lines."},
                    "offset": {"type": "integer", "description": "First line, 1-based."},
                    "limit": {"type": "integer", "description": "Maximum lines."}
                },
                "required": ["path"]
            }
        },
        {
            "name": "grep",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "glob": {"type": "string"},
                    "case_insensitive": {"type": "boolean"},
                    "context": {"type": "integer"},
                    "max_results": {"type": "integer"},
                    "output_mode": {"type": "string", "enum": ["content", "files", "count"]}
                },
                "required": ["pattern"]
            }
        },
        {
            "name": "edit",
            "inputSchema": {
                "type": "object",
                "properties": {"input": {"type": "string"}},
                "required": ["input"]
            }
        },
        {
            "name": "bash",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout": {"type": "integer", "description": "Seconds, default 120, max 600."},
                    "background": {"type": "boolean"}
                },
                "required": ["command"]
            }
        },
        {
            "name": "background",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["list", "read", "wait", "stop"]},
                    "id": {"type": "string"},
                    "offset": {"type": "integer"},
                    "timeout": {"type": "integer", "description": "Seconds for wait, default 60, max 600."}
                },
                "required": ["action"]
            }
        },
        {
            "name": "write",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }
        },
        {
            "name": "find",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "grep_keywords": {"type": "array", "items": {"type": "string"}},
                    "path": {"type": "string"},
                    "mode": {"type": "string", "enum": ["search", "outline", "architecture", "trace"]},
                    "limit": {"type": "integer"}
                }
            }
        },
        {
            "name": "todo",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "set", "add", "update", "remove", "clear"],
                        "description": "read the list, set it whole, add items, update items by id, remove ids, or clear it."
                    },
                    "items": {
                        "type": "array",
                        "description": "For set and add.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"type": "string"},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"]}
                            },
                            "required": ["content"]
                        }
                    },
                    "after": {"type": "string", "description": "For add: insert after this id."},
                    "updates": {
                        "type": "array",
                        "description": "For update.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string"},
                                "content": {"type": "string"},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"]}
                            },
                            "required": ["id"]
                        }
                    },
                    "ids": {
                        "type": "array",
                        "description": "For remove.",
                        "items": {"type": "string"}
                    }
                },
                "required": ["action"]
            }
        },
        {
            "name": "websearch",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "count": {"type": "integer"},
                    "freshness": {"type": "string", "enum": ["day", "week", "month", "year"]},
                    "domains": {"type": "array", "items": {"type": "string"}},
                    "provider": {"type": "string", "enum": ["auto", "brave", "exa", "deepseek"]},
                    "fresh": {"type": "boolean"}
                },
                "required": ["query"]
            }
        },
        {
            "name": "ctx_search",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "queries": {"type": "array", "items": {"type": "string"}, "description": "One or more search strings."},
                    "source": {"type": "string", "description": "Optional ctx: handle to scope the search to one stored output."},
                    "limit": {"type": "integer", "description": "Matches per query (default 3)."}
                },
                "required": ["queries"]
            }
        },
        {
            "name": "skill_search",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "One or more words to rank skills by."},
                    "limit": {"type": "integer", "description": "Matches to return (default 5, max 20)."}
                },
                "required": ["query"]
            }
        },
        {
            "name": "skill_manage",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ops": {
                        "type": "array",
                        "description": "Operations applied in order, all or nothing.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": {"type": "string", "enum": ["create", "patch", "write_file", "remove_file"], "description": "The operation."},
                                "name": {"type": "string", "description": "Skill name."},
                                "scope": {"type": "string", "enum": ["global", "project"], "description": "create only: where the new skill lands; global by default."},
                                "content": {"type": "string", "description": "create: full SKILL.md. patch: whole-file replacement, last resort. write_file: file text."},
                                "path": {"type": "string", "description": "patch and write_file: a file under references/, templates/, scripts/ or assets/; patch may omit it to target SKILL.md."},
                                "old_string": {"type": "string", "description": "patch: exact text to replace; must match once unless replace_all."},
                                "new_string": {"type": "string", "description": "patch: replacement text."},
                                "replace_all": {"type": "boolean", "description": "patch: replace every match."}
                            },
                            "required": ["op", "name"]
                        }
                    }
                },
                "required": ["ops"]
            }
        },
        {
            "name": "prompt_propose",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Prompt id, for example tool.edit.description."},
                    "text": {"type": "string", "description": "The complete replacement text. Omit text and reason to read the active text."},
                    "reason": {"type": "string", "description": "What failed and why this text fixes it for every Session."}
                },
                "required": ["id"]
            }
        },
        {
            "name": "retry_call",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "Failed call number from [failed call #N]."},
                    "set": {
                        "type": "object",
                        "description": "Top-level argument fields to overwrite (set a key to null to remove it).",
                        "additionalProperties": true
                    },
                    "patches": {
                        "type": "array",
                        "description": "Sequential exact-once { old, new } string replacements applied across string arguments.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old": {"type": "string"},
                                "new": {"type": "string"}
                            },
                            "required": ["old", "new"]
                        }
                    }
                },
                "required": ["id"]
            }
        },
        {
            "name": "execute",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": {"type": "string", "description": "The Python script to run."},
                    "timeout": {"type": "integer", "description": "Seconds, default 120, max 600."}
                },
                "required": ["code"]
            }
        }
    ]);
    serde_json::from_value(specs).unwrap_or_default()
}

const MAX_CAPTURED_BYTES: usize = 256 * 1024;

fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub(crate) fn strip_meta_keys(args: &Value) -> Value {
    let Some(obj) = args.as_object() else {
        return args.clone();
    };
    let mut clean = obj.clone();
    clean.remove("toolAction");
    clean.remove("toolSummary");
    Value::Object(clean)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolConfig;

    #[tokio::test]
    async fn captures_mcp_outputs_only_when_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("hello.txt"), "hello world\n").unwrap();

        let spill_off = dir.path().join("spill_off");
        let tb_off = Arc::new(Toolbox::new(ToolConfig::new(&root, &spill_off)).unwrap());
        let srv_off = McpServer::new(tb_off);
        let mut args = Map::new();
        args.insert("path".into(), json!("hello.txt"));
        let _ = srv_off.call("read", Some(args.clone())).await;
        assert!(!spill_off.join("mcp_outputs").exists());

        let spill_on = dir.path().join("spill_on");
        let mut cfg = ToolConfig::new(&root, &spill_on);
        cfg.capture_mcp_output = true;
        let tb_on = Arc::new(Toolbox::new(cfg).unwrap());
        let srv_on = McpServer::new(tb_on);
        let _ = srv_on.call("read", Some(args)).await;

        let files: Vec<_> = std::fs::read_dir(spill_on.join("mcp_outputs"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(files.len(), 1);
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(files[0].path()).unwrap()).unwrap();
        assert_eq!(saved["name"], "read");
        assert_eq!(saved["args"], json!({"path": "hello.txt"}));
        assert!(saved["output"].as_str().unwrap().contains("hello world"));
    }
}
