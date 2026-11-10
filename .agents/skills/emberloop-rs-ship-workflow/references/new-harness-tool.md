# Adding a harness tool or a prompt id

A new tool or prompt id touches many places. Miss one and a pinned test or a live Session fails.

1. Prompt id: add the const, the `PROMPT_IDS` entry and the `default_text` arm in `crates/prompts/src/lib.rs`; add the `include_str!` const in `defaults.rs`; add the `prompts/<id>.md` file. A tool's id is `tool.<name>.description`.
2. Prompt text is validated for placeholders when the store loads. A literal double-brace token in a default file (even as an example) is an `UnknownPlaceholder` and fails every pin test. Describe placeholders in words.
3. Tool: add `crates/tools/src/<name>.rs`, `pub mod` (alphabetical), the `call_inner` arm in `crates/tools/src/lib.rs` and the spec in `tool_specs` in `mcp.rs`. A schema `required` list holds only what every call needs.
4. Pinned lists: add the name to `OFFERED_TOOLS` in `crates/prompts/src/tests.rs` and to the expected MCP name list in `crates/tools/tests/tools.rs`.
5. Config the tool needs goes on `ToolConfig` (default `None`, an error message that names the missing config). Set it in all three daemon builders: `mcp_tool_config`, `toolbox_for` and `review.rs` `ReviewExecutor`.
6. The background review forks the Session request, so a tool declared to every Session is callable there only if `ALLOWED` in `crates/daemon/src/review.rs` lists it. The review's `read` is jailed to the Project and the global skills root, so give the tool a way to return any data the review must see.
7. API route: add the DTO to the `components` list and the handler to the `paths` list in `routes.rs`, then register the route. Rerun `scripts/check` and stage the regenerated `openapi.json` and `schema.d.ts`.
8. Update the `AGENTS.md` of each crate touched.
