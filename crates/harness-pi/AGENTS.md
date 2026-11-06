# AGENTS.md — crates/harness-pi

## Purpose

`pi --mode acp` (pi-rust) harness implementation for `opencode-go` and the API-key providers of `emberloop_core::providers`:
- `PiHarness` writes a per-Session `HOME` under `<data root>/pi-homes/<session>` containing `.pi/agent/models.json` (one entry per saved key in `<config root>/keys.json`: `opencode-go` at `<proxy>/v1`, a registry provider at `<proxy>/x/<id><path>`; mode `0600`; a model of a provider with no saved key is refused before pi starts) and `.pi/agent/mcp.json` (`exposure: direct`, `timeout: 600` seconds so pi's 60 s MCP default does not kill a long tool call, the value pig writes on its own entry), then launches `pi --mode acp --no-builtin-tools --offline` under `env -i` with only `HOME` and `PATH`.
- `PiHarness::models` opens a throwaway `_catalogue` ACP session and flattens the `model` config option into `(id, label)` pairs for the model catalogue.
- `fake/fake_pi.rs` (`fake-pi` binary) is the in-process ACP fake driving `tests/pi.rs`.

## Child DOX Index

(none)
