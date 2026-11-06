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
}

impl Clone for McpServer {
    fn clone(&self) -> Self {
        Self {
            toolbox: Arc::clone(&self.toolbox),
        }
    }
}

impl McpServer {
    pub fn new(toolbox: Arc<Toolbox>) -> Self {
        Self { toolbox }
    }

    pub fn tools(&self) -> Vec<Tool> {
        tool_specs()
            .into_iter()
            .filter_map(|spec| {
                let description = spec.get("description")?.as_str()?.to_string();
                let name = spec.get("name")?.as_str()?.to_string();
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

fn tool_specs() -> Vec<Value> {
    let specs = json!([
        {
            "name": "read",
            "description": "Read a file with numbered lines and a 4-hex snapshot tag for edit. Large output spills to a ctx: handle.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File, directory or ctx:<handle> pointer."},
                    "offset": {"type": "integer", "description": "First line, 1-based."},
                    "limit": {"type": "integer", "description": "Maximum lines."}
                },
                "required": ["path"]
            }
        },
        {
            "name": "grep",
            "description": "Search files with regex, respecting .gitignore. Hits feed the seen-line store.",
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
            "description": "Apply line-anchored hashline edits. Header [path#TAG], ops PUT N.=M:, PUT <N:, PUT >N:, PUT >$:, CUT N, CUT N.=M; body lines start with '+'.",
            "inputSchema": {
                "type": "object",
                "properties": {"input": {"type": "string"}},
                "required": ["input"]
            }
        },
        {
            "name": "bash",
            "description": "Run a shell command in the project root. Own process group, timeout, capped output.",
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
            "description": "Manage background commands: list, read the log at an offset, wait, or stop.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["list", "read", "wait", "stop"]},
                    "id": {"type": "string"},
                    "offset": {"type": "integer"},
                    "timeout": {"type": "integer"}
                },
                "required": ["action"]
            }
        }
    ]);
    serde_json::from_value(specs).unwrap_or_default()
}
