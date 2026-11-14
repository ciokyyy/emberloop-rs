use std::path::Path;

use crate::{ToolError, ToolResult, Toolbox};

type Renderer = fn(&Toolbox) -> String;

const TOPICS: &[(&str, &str, Renderer)] = &[
    (
        "mcp",
        "MCP server configuration (`mcp.json`) and `execute` bridge usage",
        render_mcp,
    ),
    (
        "config",
        "Active paths, `settings.json`, `env`, `tool_keys`, `prompts`, and `skills` conventions",
        render_config,
    ),
];

#[derive(Debug, PartialEq, Eq)]
pub enum DocsTarget<'a> {
    Index,
    Topic(&'a str),
}

pub fn parse_target(raw: &str) -> Option<DocsTarget<'_>> {
    let rest = raw.trim().strip_prefix("el://docs")?;
    if rest.is_empty() {
        return Some(DocsTarget::Index);
    }
    let rest = rest.strip_prefix('/')?.trim_matches('/');
    if rest.is_empty() {
        return Some(DocsTarget::Index);
    }
    Some(DocsTarget::Topic(rest))
}

pub fn read(toolbox: &Toolbox, target: DocsTarget<'_>) -> ToolResult<String> {
    let unknown = match target {
        DocsTarget::Index => return Ok(render_index()),
        DocsTarget::Topic(name) => name,
    };
    match TOPICS.iter().find(|(name, _, _)| *name == unknown) {
        Some((_, _, render)) => Ok(render(toolbox)),
        None => {
            let valid = TOPICS
                .iter()
                .map(|(name, _, _)| *name)
                .collect::<Vec<_>>()
                .join(", ");
            Err(ToolError::msg(format!(
                "unknown documentation topic {unknown:?}; valid topics: {valid}"
            )))
        }
    }
}

fn render_index() -> String {
    let mut out = String::from(
        "# Emberloop Documentation Topics\n\nRead a topic with `read(path=\"el://docs/<topic>\")`:\n\n",
    );
    for (idx, (name, summary, _)) in TOPICS.iter().enumerate() {
        out.push_str(&format!("{}. `{name}` — {summary}\n", idx + 1));
    }
    out
}

fn display_path(path: Option<&Path>) -> String {
    path.map_or_else(
        || "(not configured)".to_string(),
        |p| p.display().to_string(),
    )
}

fn render_mcp(toolbox: &Toolbox) -> String {
    let mcp_file = display_path(toolbox.config.mcp_file.as_deref());
    let root = toolbox.root().display();
    format!(
        r#"# MCP Server Configuration (`mcp.json`)

- Active `mcp.json`: `{mcp_file}`
- Active Project root: `{root}`

## File format

`mcp.json` uses a Claude-compatible `mcpServers` object. Emberloop only launches stdio child processes; entries specifying `"url"` are refused. Each connected server runs in its own process group and terminates when the Session's tool server stops.

```json
{{
  "mcpServers": {{
    "graph": {{
      "command": "npx",
      "args": ["-y", "codebase-memory-mcp@0.12.1"],
      "env": {{
        "CBM_CACHE_DIR": "/path/to/cache"
      }},
      "include": ["search_graph", "trace_path"],
      "exclude": [],
      "enabled": true,
      "summary": "Codebase knowledge graph",
      "project": "optional-override",
      "setup": [["binary", "--init"]]
    }}
  }}
}}
```

### Supported entry fields

- `command` (string): executable to launch via stdio (`url` is rejected).
- `args` (string[]): command-line arguments.
- `env` (object): additional environment variables for that server process only.
- `include` (string[]): optional tool allowlist (empty = all server tools).
- `exclude` (string[]): optional tool denylist applied after `include`.
- `enabled` (bool, default `true`): set `false` to skip connecting the server.
- `summary` (string): description returned by `tools.servers()` inside `execute`.
- `project` (string): optional project name override for the `graph` server.
- `setup` (string[][]): argv commands run once per `CBM_CACHE_DIR` before first start.

## Using MCP tools from `execute`

MCP servers are never exposed as top-level harness tools; call them inside `execute`:

- `tools.<server>.<tool>(**kwargs)` or `tools.call("<server>.<tool>", **kwargs)`
- `tools.<tool>(**kwargs)` works as a bare alias when only one server defines `<tool>`
- `tools.servers()` lists configured servers and connection status
"#
    )
}

fn render_config(toolbox: &Toolbox) -> String {
    let mcp_file = display_path(toolbox.config.mcp_file.as_deref());
    let env_file = display_path(toolbox.config.env_file.as_deref());
    // Intentionally format only the file path; never read secret key files or env secret values.
    let tool_keys_file = display_path(toolbox.config.tool_keys_file.as_deref());
    let prompts_dir = display_path(toolbox.prompts_dir());
    let global_skills = display_path(toolbox.global_skills());
    let project_skills = toolbox.root().join(".agents").join("skills");

    format!(
        r#"# Active Emberloop Configuration

## Active paths (non-secret)

- `mcp.json`: `{mcp_file}`
- Runtime `env`: `{env_file}` (also exported in shells as `$EMBERLOOP_ENV_FILE`)
- `tool_keys` file: `{tool_keys_file}` (path only; contents are never read or exposed here)
- `prompts` directory: `{prompts_dir}`
- Global `skills` root: `{global_skills}`
- Project `skills` root: `{}`

## Conventions

### `settings.json`

Stored at `<data root>/settings.json` beside the `sessions/` directory and `projects.json`; a save rewrites the whole file. Keys (snake_case) include `default_model`, `title_generation`, `title_model`, `spec_reviewer` and `standards_reviewer` (fallback ladders), `subagent_tiers` (`smol`, `mid`, `top`, one fallback ladder each), `proxy_compression`, the compression thresholds `max_context_limit`, `min_pressure_benefit_tokens`, `emergency_threshold_percent`, `preserve_recent_messages`, `protected_tools`, `inject_tool`, `inject_nudge`, `tier2_trigger` and `tier3_trigger`, `ctx_spill_enabled`, `skills_review_interval`, and the `todo_*` toggles.

### Runtime `env`

Shell script sourced before every `bash` and `execute` command, including `background: true`. Use it for `PATH` additions and non-secret tool environment variables so updates apply on the next call without restarting the daemon. Do not store API keys here.

### `tool_keys`

The `tool_keys` file (`tool-keys.json` by default) is a JSON object mapping key names such as `BRAVE_API_KEY`, `EXA_API_KEY` or `JINA_API_KEY` to secrets. `Toolbox::tool_key` reads it on demand for `websearch` and URL fetches. These docs expose only the file path and never read its contents or environment secret values.

### `prompts`

Prompt overrides live in `<prompts dir>/<id>.md`; the sibling `prompt-proposals/` directory holds pending proposals from `prompt_propose`. A Session pins the prompt text it renders when it starts, so editing an override affects Sessions started afterwards, never a running one.

### `skills`

Skills are folders containing `SKILL.md` with YAML frontmatter (`name`, `description`, optional `metadata.origin`). A project skill under `<project>/.agents/skills` wins over a global skill of the same name.
"#,
        project_skills.display()
    )
}
