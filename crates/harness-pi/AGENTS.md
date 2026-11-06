# AGENTS.md — crates/harness-pi

## Purpose

`pi --mode acp` (pi-rust) harness implementation for the `opencode-go` provider:
- `PiHarness` writes a per-Session `HOME` under `<data root>/pi-homes/<session>` containing `.pi/agent/models.json` (proxy base URL and the `opencode-go` key from `<config root>/keys.json`, mode `0600`) and `.pi/agent/mcp.json` (`exposure: direct`, `timeout: 600` seconds so pi's 60 s MCP default does not kill a long tool call, the value pig writes on its own entry), then launches `pi --mode acp --no-builtin-tools --offline` under `env -i` with only `HOME` and `PATH`.
- `PiHarness::models` opens a throwaway `_catalogue` ACP session and flattens the `model` config option into `(id, label)` pairs for the model catalogue.
- `fake/fake_pi.rs` (`fake-pi` binary) is the in-process ACP fake driving `tests/pi.rs`.

## Child DOX Index

(none)
