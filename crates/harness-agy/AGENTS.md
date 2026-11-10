# AGENTS.md — crates/harness-agy

## Purpose

Implementation of the `Harness` trait for Google Antigravity ACP server (`agy_acp_server.par`), confined with `enabledTools: []`, empty working directory, eager `el` MCP server, and proxy environment wiring.

`agent-client-protocol` v1 drives the process: `initialize` with session capabilities, `session/new` with the `el` stdio MCP server, `session/prompt`, `session/cancel`, `session/load`, and `session/delete`. Permission requests are auto-approved with an allow option. Tool-call updates and message chunks map to `HarnessEvent`; `stop_reason` is serialized verbatim.

The `el` server gets `EMBERLOOP_MAX_CALL_SECS=170`: the Antigravity server aborts an MCP call that blocks 3 minutes (`MCP tool call to server "el" timed out after 3m0s`), and a Session then ends with `Harness process exited unexpectedly (WS close code 1011)`. The tools clamp their waits below that.

## Child DOX Index

(none)
