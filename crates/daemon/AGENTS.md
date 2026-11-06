# AGENTS.md — crates/daemon

## Purpose

Composition root and binary for the whole system.
- `Config` and `Cli` (clap) carry `--port` (3000), `--host` (127.0.0.1), `--data-root`, `--config-root`, `--upstream`, and `--allow-remote`. A non-loopback host is rejected without `--allow-remote`; the accepted bind logs a warning.
- `Daemon::open` loads `DataRoot`, `ProjectsStore`, `SessionsStore`, a `PromptStore` rooted at `<config root>/prompts`, and starts the prompt watcher. The proxy and API share one `EventBus`; the API gets the config-root prompt store through `AppState::with_prompts`, and `Daemon::open` starts the model catalogue refresher at boot.
- `router` merges the REST/OpenAPI router and the proxy router, then serves `web/build` through `rust-embed`. Unknown non-API paths fall back to `index.html`; unknown `/api` and `/p` paths stay 404.
- `toolbox_for` and `mcp_server_for` build a Session's tools from its Project root and spill dir. `run_mcp` serves those tools over MCP stdio for the `mcp` subcommand, which `harness_wiring` puts in `HarnessWiring.mcp_command` along with a signed `/p/<token>` proxy URL.
- `HarnessPlugin` wires both `AgyHarness` (`EMBERLOOP_AGY_BIN` or `<data root>/agy-acp/1.4.5/agy_acp_server.par`) and `PiHarness` (`EMBERLOOP_PI_BIN` or `<data root>/pi/v0.12.11/pi`), routing `opencode-go/*` models to `PiHarness` and others to `AgyHarness`; `PiSource` feeds `pi`'s `opencode-go` models into `ModelCatalogue`, and `AgyLogin::with_keys` manages `<config root>/keys.json`.
- `serve` binds the configured host, serves until SIGINT or SIGTERM, and the watcher stops when the daemon drops.

## Tests

`SessionRunner` stores the harness `resume_token` in `meta.json`, passes it back with the stable `<session>/agent-work` directory on the next spawn, and resets the proxy history (splices, compressed blocks) when the harness conversation started over. `tests/acceptance.rs` covers Ticket 11 checks 1, 2, 7, 8 against a fake upstream: Project/Session persistence, token rejection and acceptance, prompt pinning across a config edit, background PID adoption across a daemon restart, embedded SPA fallback, remote-bind validation, and harness wiring.

## Child DOX Index

(none)
