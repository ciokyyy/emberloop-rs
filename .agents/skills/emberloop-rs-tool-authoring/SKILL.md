---
name: emberloop-rs-tool-authoring
description: Use when adding or changing a built-in emberloop-rs tool in crates/tools (new MCP tool, input schema, model-facing description, execute bridge, or docs view). Wires every surface in one pass so the advertised list, contract tests and prompt store stay consistent.
metadata:
  origin: agent
---

# Authoring a bundled emberloop-rs tool

One tool lives in five places. Miss one and the tool is invisible, unreachable from `execute`, or fails `cargo test -p tools` on the exact advertised list.

## Procedure

1. **List every surface before editing.**
   - `crates/tools/src/mcp.rs` `tool_specs()`: name plus inline `inputSchema` (and the `read.path`-style description text).
   - `crates/tools/src/lib.rs`: a `dispatch_inner` arm for anything the `execute` bridge may call; an arm in public `call` only for a tool that must stay unreachable from inside `execute`.
   - `crates/tools/src/execute.rs` `_SPECS`: the schema copy that exposes the tool to `tools.<name>` in scripts.
   - `crates/prompts/prompts/tool.<name>.md`: the model-facing body.
   - `crates/tools/AGENTS.md`: one line on the module or behaviour.
2. **Register the description text.** `PROMPT_IDS` and `default_text` in `crates/prompts/src/lib.rs`, the `include_str!` entry in `crates/prompts/src/defaults.rs`, and the name in `OFFERED_TOOLS` in `crates/prompts/src/tests.rs`. A prompt file that exists on disk but is unregistered never reaches the model: descriptions flow through `prompts::tool_name` into `McpServer::with_descriptions`.
3. **Keep the prompt inside the contract.** `crates/tools/tests/contract.rs` builds its tool list from `ToolConfig::new`, where `session_url` is `None`, so conditional tools are absent from the advertised set. In a tool prompt, put backticks only around schema property names, enum values and advertised tool names; a backticked unadvertised tool fails the contract test. Every top-level schema parameter must appear in backticks in the prompt.
4. **Update the exact advertised list.** `mcp_serves_tools_and_errors` in `crates/tools/tests/tools.rs` asserts the full name vector in declaration order; add the new name where `tool_specs()` declares it.
5. **Test at two levels.** One Toolbox-level test and one `McpServer::call` test covering the happy path, each documented error string, and any state the tool writes. A persisted or retried call needs a second call proving the state is re-read, not cached.
6. **Verify.** `cargo fmt -p prompts -p tools && cargo clippy -p prompts -p tools --all-targets -- -D warnings && cargo test -p prompts -p tools`, then the repo gate (`./scripts/check`).

## Pitfalls

- A tool that re-dispatches another tool (retry a stored call, replay an edited request) must call the public `Toolbox::call`, not `call_inner`. An arm that exists only in `call` (`execute` is the current one) is unreachable from the inner dispatcher, so a record naming that tool dies with `unknown tool`.
- A tool whose future can reach itself through another dispatcher forms a recursive async type; return `Pin<Box<dyn Future<Output = ToolResult<T>> + Send + 'a>>` and keep an explicit recursion guard (a thread-local set for the duration of `execute`). The comment on the boxed signature names `Toolbox::call` as the reason.
- Wrap failures only after the denied/allowlist check. Wrapping before it records retry state for a call that can never be permitted, and the model follows a dead `#N`.
- Any rendered or schema-adjacent reference text (examples, paths, settings keys, pinned dependency versions, sibling directory names) is an accuracy surface: grep each concrete name against the code that owns it, and give every required value its own assertion. A model that follows a wrong key writes invalid config.
- Never read or echo a secret file from a tool. Render the configured path only, and make the test write a known secret string and assert it is absent from the output.
- Config fields that exist only for tests (an allowlist with no setter, a path with no production writer) are speculative generality unless a real caller sets them; name them in the reply instead of adding them.
