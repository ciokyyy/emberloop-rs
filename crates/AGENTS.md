# AGENTS.md — crates

## Purpose

Cargo workspace containing all library and binary crates for emberloop-rs.

## Child DOX Index

- `crates/core/AGENTS.md` — Core primitives: Context tree, typed EventBus, ID generation, shared provider endpoint registry
- `crates/store/AGENTS.md` — Project and Session persistence, append-only event log
- `crates/prompts/AGENTS.md` — Prompt files: defaults, overrides, placeholders, per-Session pinning
- `crates/proxy/AGENTS.md` — Model-wire proxy: routing, usage meter and context-category estimates, splices, Compressor, prompt-cache prefix stability
- `crates/harness/AGENTS.md` — Harness trait, acknowledged live model configuration and wiring types
- `crates/harness-acp/AGENTS.md` — Shared ACP session driver
- `crates/harness-agy/AGENTS.md` — agy ACP harness implementation
- `crates/harness-pi/AGENTS.md` — pi-rust ACP harness implementation
- `crates/harness-claude/AGENTS.md` — Claude Code stream-json harness: resume, init, stderr tail, fake CLI, live test
- `crates/api/AGENTS.md` — REST API, model/effort and image validation, model catalogue, SSE streaming, OpenAPI schema
- `crates/tools/AGENTS.md` — MCP tool server: read, grep, hashline edit, bash, background, jail and spill store
- `crates/daemon/AGENTS.md` — daemon composition root, CLI, embedded SPA, MCP and harness wiring, acceptance tests
