# AGENTS.md — crates/harness-pi

## Purpose

`pi --mode acp` (pi-rust) harness implementation for `opencode-go` and the API-key providers of `emberloop_core::providers`:
- `PiHarness` writes a per-Session `HOME` under `<data root>/pi-homes/<session>` containing `.pi/agent/models.json` (an entry for each OAuth endpoint from `OAUTH_PROVIDERS`, with only its signed proxy `baseUrl`, plus one entry per saved key in `<config root>/keys.json`: `opencode-go` at `<proxy>/v1`, a registry provider at `<proxy>/x/<id><path>`; mode `0600`; a model of a provider with no saved key is refused before pi starts) and `.pi/agent/mcp.json` (`exposure: direct`, `timeout: 600` seconds so pi's 60 s MCP default does not kill a long tool call, the value pig writes on its own entry), then launches `pi --mode acp --no-builtin-tools --offline` under `env -i` with only `HOME` and `PATH`.
- Each HOME gets `settings.json` with `transport: sse`, preserving other settings and reapplying mode `0600` to existing private files, so the HTTP proxy sees the whole conversation instead of a cached WebSocket delta. `models.json` is rewritten on every open, including an OAuth-only or resumed Session: Codex uses `<proxy>/x/openai-codex/backend-api`, then pi appends `/codex/responses`; no `apiKey` is invented for it. OAuth resolution and refresh remain with pi using its private `auth.json` copy. Already-running children retain their loaded configuration until reconnect.
- The stdio MCP entry receives the explicit `HarnessWiring.env` map, including `EMBERLOOP_SESSION_URL` and `EMBERLOOP_REVIEWER`, so reviewer tools retain Session identity without exposing the daemon environment to pi. The pi process itself still receives only `HOME` and `PATH`.
- `catalogue.rs` / `PiHarness::catalogue` uses a separate `_metadata` HOME and an offline RPC child to read full model metadata and query each reasoning model’s available thinking levels after selecting it. Only chat models of saved-key providers or a connected Codex account are offered. Windows and image support come from pi, never blanket defaults; no connected providers returns an empty list without spawning. The child has a scrubbed environment, a 30-second total bound and is killed/reaped after the query; real Session state is untouched.
- `PiHarness::models` opens a throwaway `_catalogue` ACP session and flattens the `model` config option into `(id, label)` pairs for the model catalogue.
- `fake/fake_pi.rs` (`fake-pi` binary) is the ACP/RPC fake driving `tests/pi.rs`.

## Child DOX Index

(none)
