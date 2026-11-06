# AGENTS.md — crates/harness-agy

## Purpose

Implementation of the `Harness` trait for Google Antigravity ACP server (`agy_acp_server.par`), confined with `enabledTools: []`, empty working directory, eager `el` MCP server, and proxy environment wiring.

`agent-client-protocol` v1 drives the process: `initialize` with session capabilities, `session/new` with the `el` stdio MCP server, `session/prompt`, `session/cancel`, `session/load`, and `session/delete`. Permission requests are auto-approved with an allow option. Tool-call updates and message chunks map to `HarnessEvent`; `stop_reason` is serialized verbatim.

## Child DOX Index

(none)
