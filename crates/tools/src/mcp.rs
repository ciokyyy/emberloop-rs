use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{json, Map, Value};

use crate::Toolbox;

/// MCP server over rmcp: advertises the workspace tools and dispatches calls.
pub struct McpServer {
    toolbox: Arc<Toolbox>,
    descriptions: Arc<HashMap<String, String>>,
}

impl Clone for McpServer {
    fn clone(&self) -> Self {
        Self {
            toolbox: Arc::clone(&self.toolbox),
            descriptions: Arc::clone(&self.descriptions),
        }
    }
}

impl McpServer {
    /// A server that advertises the embedded Prompt Store texts.
    pub fn new(toolbox: Arc<Toolbox>) -> Self {
        Self {
            toolbox,
            descriptions: Arc::new(default_descriptions()),
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
                let description = self.descriptions.get(&name).cloned().unwrap_or_default();
                let schema: Map<String, Value> =
                    serde_json::from_value(spec.get("inputSchema")?.clone()).ok()?;
                Some(Tool::new(name, description, Arc::new(schema)))
            })
            .collect()
    }

    pub async fn call(&self, name: &str, arguments: Option<Map<String, Value>>) -> CallToolResult {
        let args = arguments.map_or_else(|| json!({}), Value::Object);
        match self.toolbox.call(name, &args).await {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
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
            "name": "read",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File, directory, http(s) URL or ctx:<handle> pointer."},
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
