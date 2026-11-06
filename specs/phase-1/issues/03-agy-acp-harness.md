# Ticket 03: agy ACP driver

## Goal

Implement the `Harness` trait for `agy` using the official `agent-client-protocol`, supporting `spawn`, `send`, `events`, `cancel`, `resume`, `wake`. Blocked by ticket 02.

## Status

Completed. PASS.

## Deliverables

1. **`crates/harness` (`HarnessSession` / `Harness` traits)**:
   - `HarnessWiring`: `proxy_url`, `mcp_url`, `mcp_command`, `env`.
   - `HarnessSession`: `send(prompt, images)`, `events() -> EventStream`, `cancel()`, `resume(token)`, `wake(text)`.
   - `Harness`: `spawn(session_id, wiring)`.
2. **`crates/harness-agy`**:
   - Spawns `agy_acp_server.par --uid=` with `GEMINI_HOME` and `AGY_ACP_CCPA_BASE_URL`.
   - Confines session with empty directory, `enabledTools: []`, and eager MCP `el`.
   - Speaks ACP stdio JSON-RPC (`initialize`, `session/new`, `session/set_config_option`, `session/prompt`, `session/cancel`, `session/load`).
   - Streams events over broadcast channel into `EventStream`.
