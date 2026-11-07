# AGENTS.md — crates/api

## Purpose

Exposes axum REST API, SSE event endpoints and the model catalogue with utoipa OpenAPI schema generator:
- `/api/projects`: list, create, update, delete projects
- `/api/sessions`: list, create, update, delete sessions. PATCH accepts `model` and `thoughtLevel` together; `session_setup.rs` validates catalogue efforts and idle state, waits for the driver acknowledgment, then persists settings. Unknown models/efforts are 400; busy/refused changes are 409 and keep metadata unchanged.
- `/api/sessions/:id/prompt`: user prompt with text and images; refreshes `lastUsedAt`. The prompt route allows 70 MiB JSON bodies for up to five 10 MiB PNG/JPEG/WebP/GIF attachments, rejects known text-only models, and returns 409 for images while working. While the Session's turn is active a text-only prompt is queued as a steer instead, so a reloaded UI that thinks the Session is idle cannot hand the harness a second prompt.
- `/api/sessions/:id/usage`: prompt-cache history, one `UsageRecord` per request from the Session's `usage` events, oldest first; `?limit=` keeps the newest N (default 200, max 1000). `inputTokens` is the whole prompt, `freshInputTokens` the uncached part (`input - cached - creation`), `hitPercent` is `cached / prompt * 100`, null for an empty prompt. Built in `src/usage.rs` by replaying the log.
- `/api/sessions/:id/steer`: steer input queue
- `/api/sessions/:id/cancel`: cancel current execution
- `/api/sessions/:id/events`: SSE event stream with `?after=<seq>` replay. A first load (`?limit=N`, no cursor) sends the tail window, then one named `snapshot` frame (`{status, usage, todos}`: the newest status, usage (including the proxy breakdown) and todo-list event data, however old), so a long working Session does not open as idle and its plan card survives a reload. The frame has no seq and never appears in history pages.
- `/api/tool-keys`: `GET` lists `brave`, `exa` and `jina` with the shell variable each sets and whether it is set (never the value); `PUT /api/tool-keys/{name}` with `{ value }` stores or, on an empty value, clears that key in `<config root>/tool-keys.json` (`store::tool_keys`, 0600); the tools read that file on every call, so no environment variable and no restart are involved. Unknown names are 404; a daemon without a config root answers 409.
- `/api/settings`: get and update system settings, including the ticket-20 Context keys (`max_context_limit`, `min_pressure_benefit_tokens`, `emergency_threshold_percent`, `preserve_recent_messages`, `protected_tools`, `inject_tool`, `inject_nudge`, `tier2_trigger`, `tier3_trigger`); `serde(default)` fills them for older saved settings.
- `/api/prompts`: list prompt ids with default/override/active hashes; `PUT/DELETE /api/prompts/:id` writes or removes an override
- `/api/models`: merged model catalogue; `POST /api/models/refresh` forces the configured sources to refresh
- `/api/providers`: list providers, Antigravity login flow, and `PUT`/`DELETE /api/providers/:id/key` for API-key providers (`opencode-go` and the `emberloop_core::providers` registry; any other id is 404). `ProviderView.auth` is `login` or `key`
- `/api/version`: reports `EMBERLOOP_BUILD_VERSION` when supplied at build time, otherwise the crate version, matching the daemon release version.
- `/openapi.json`: OpenAPI 3.0 specification
- `AppState::new` reads prompts from `<data root>/prompts`; `AppState::with_prompts` and `AppState::with_models` let the daemon pass the config-root prompt store and multi-source model catalogue.

## Child DOX Index

(none)

## Model catalogue

`src/models.rs` owns `ModelEntry`, the async `Source` trait and `ModelCatalogue`. Sources merge by `id` in source order. A failing source keeps its last good entries, marked `stale`, and records the error; `AgySource` is the phase-1 static source. The snapshot is `<data root>/models/catalogue.json`, written by temp file and rename and loaded before the first refresh. The daemon calls `spawn_refresher` at boot; it refreshes immediately and every 10 minutes. The catalogue is in `AppState::models`, and `ModelCatalogue::window` answers a model's context window for the proxy's usage meter.

## Title and usage contracts

Settings include `title_generation` (default true) and `title_model` (default `openrouter/openrouter/free`). PUT validates the provider/model format. Settings remain process-local, like the existing settings. `GET /api/providers/{provider}/usage?refresh=true` calls the daemon usage seam and returns `UsageView { windows, balance, error }`; percentages describe used quota, resets are epoch-second strings or provider timestamps. No credentials are returned. PATCH applies explicit title changes through `SessionsStore::update_title`.
