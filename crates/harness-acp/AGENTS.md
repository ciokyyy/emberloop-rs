# AGENTS.md — crates/harness-acp

## Purpose

The ACP driver every ACP harness shares: `start(AcpLaunch, session_id)` spawns the agent process, runs `initialize`, `session/new`, and the `model` + `thought_level` config picks, then serves `AcpSession` (a `HarnessSession`). Permission requests are auto-approved with an allow option; tool-call updates (including streamed `InProgress` inputs and unwrapped MCP text outputs), thought chunks (`AgentThoughtChunk`), and message chunks map to `HarnessEvent`; `stop_reason` is serialized verbatim. A harness crate supplies only the launch: process config, cwd, MCP servers, meta, model, thought_level, client name, and `forward_thoughts` (false when the proxy already publishes the agent's reasoning).

Live `configure_model` commands serialize `model` then optional `thought_level` through ACP config requests and return the agent’s success/refusal through a oneshot with a 10-second caller timeout. They preserve the ACP Session id. `AcpSession::select_model` uses the same request and returns the agent's updated `configOptions`, which is how a model catalogue reads the efforts each model accepts.

## Child DOX Index

(none)
