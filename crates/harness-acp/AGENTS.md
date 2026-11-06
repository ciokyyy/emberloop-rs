# AGENTS.md — crates/harness-acp

## Purpose

The ACP driver every ACP harness shares: `start(AcpLaunch, session_id)` spawns the agent process, runs `initialize`, `session/new`, and the `model` + `thought_level` config picks, then serves `AcpSession` (a `HarnessSession`). Permission requests are auto-approved with an allow option; tool-call updates (including streamed `InProgress` inputs and unwrapped MCP text outputs), thought chunks (`AgentThoughtChunk`), and message chunks map to `HarnessEvent`; `stop_reason` is serialized verbatim. A harness crate supplies only the launch: process config, cwd, MCP servers, meta, model, thought_level, and client name.

## Child DOX Index

(none)
