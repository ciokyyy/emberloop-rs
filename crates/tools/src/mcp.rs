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
        },
        {
            "name": "write",
            "description": "Write a whole file inside the project root, creating parent directories.",
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
            "description": "Locate files, symbols and line ranges implementing a concept or behavior. Modes: search, outline, architecture, trace.",
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
            "name": "readurl",
            "description": "Fetch a web URL as Markdown or plain text through a tiered reader.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "offset": {"type": "integer"},
                    "limit": {"type": "integer"}
                },
                "required": ["url"]
            }
        },
        {
            "name": "websearch",
            "description": "Search the web (Brave/Exa) with optional freshness and domain filters.",
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
            "name": "git",
            "description": "Read-only git inspection: diff, log, show, blame, status, ls-files, grep, rev-parse, merge-base, cat-file, branch and tag listing. Takes args (a list of strings) run as git -C <root> --no-pager. Writing, executing and configuring flags (-c, -O, --output, --ext-diff, --textconv, --exec, --upload-pack, --open-files-in-pager, --filters, --show-signature and their abbreviations) are refused.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "args": {"type": "array", "items": {"type": "string"}, "description": "Git arguments starting with the subcommand, e.g. [\"log\", \"--oneline\", \"-5\"]. Branch and tag need a list flag."},
                    "timeout": {"type": "integer", "description": "Seconds, default 120, max 600."}
                },
                "required": ["args"]
            }
        },
        {
            "name": "check",
            "description": "Re-run one of the repo's named checks from .emberloop/check: name (default \"default\"), args substituted into the command's {args} slot. Shell metacharacters in args are refused.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Which named check to run (default \"default\")."},
                    "args": {"type": "string", "description": "Substituted into the command's {args} slot only."}
                }
            }
        },
        {
            "name": "ctx_search",
            "description": "Search stored tool output (spilled ctx: handles): ranked snippets, each with its ctx:<id>:<start>-<end> range. Takes one or more queries, an optional source handle to scope the search, and a per-query limit (default 3). Page a hit with read ctx:<id>:<start>-<end>.",
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
            "description": "Run a Python script (stdlib only) that calls el tools via `import tools` over a local bridge.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": {"type": "string"},
                    "timeout": {"type": "integer"}
                },
                "required": ["code"]
            }
        }
    ]);
    serde_json::from_value(specs).unwrap_or_default()
}
