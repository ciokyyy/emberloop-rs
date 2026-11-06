# AGENTS.md — crates/harness-acp

## Purpose

The ACP driver every ACP harness shares: `start(AcpLaunch, session_id)` spawns the agent process, runs `initialize`, `session/new` and the `model` config pick, then serves `AcpSession` (a `HarnessSession`). Permission requests are auto-approved with an allow option; tool-call updates and message chunks map to `HarnessEvent`; `stop_reason` is serialized verbatim. A harness crate supplies only the launch: process config, cwd, MCP servers, meta, model and client name.

## Child DOX Index

(none)
