# AGENTS.md — crates/api

## Purpose

Exposes axum REST API, SSE event endpoints and the model catalogue with utoipa OpenAPI schema generator:
- `/api/projects`: list, create, update, delete projects
- `/api/sessions`: list, create, update, delete sessions
- `/api/sessions/:id/prompt`: user prompt with text and images; refreshes `lastUsedAt`. While the Session's turn is active it is queued as a steer instead, so a reloaded UI that thinks the Session is idle cannot hand the harness a second prompt.
- `/api/sessions/:id/steer`: steer input queue
- `/api/sessions/:id/cancel`: cancel current execution
- `/api/sessions/:id/events`: SSE event stream with `?after=<seq>` replay. A first load (`?limit=N`, no cursor) sends the tail window, then one named `snapshot` frame (`{status, usage, todos}`: the newest status, usage and todo-list event data, however old), so a long working Session does not open as idle and its plan card survives a reload. The frame has no seq and never appears in history pages.
- `/api/settings`: get and update system settings, including the ticket-20 Context keys (`max_context_limit`, `min_pressure_benefit_tokens`, `emergency_threshold_percent`, `preserve_recent_messages`, `protected_tools`, `inject_tool`, `inject_nudge`, `tier2_trigger`, `tier3_trigger`); `serde(default)` fills them for older saved settings.
- `/api/prompts`: list prompt ids with default/override/active hashes; `PUT/DELETE /api/prompts/:id` writes or removes an override
- `/api/models`: merged model catalogue; `POST /api/models/refresh` forces the configured sources to refresh
- `/api/providers`: list providers, Antigravity login flow, and `PUT /api/providers/opencode-go/key`
- `/openapi.json`: OpenAPI 3.0 specification
- `AppState::new` reads prompts from `<data root>/prompts`; `AppState::with_prompts` and `AppState::with_models` let the daemon pass the config-root prompt store and multi-source model catalogue.

## Child DOX Index

(none)

## Model catalogue

`src/models.rs` owns `ModelEntry`, the async `Source` trait and `ModelCatalogue`. Sources merge by `id` in source order. A failing source keeps its last good entries, marked `stale`, and records the error; `AgySource` is the phase-1 static source. The snapshot is `<data root>/models/catalogue.json`, written by temp file and rename and loaded before the first refresh. The daemon calls `spawn_refresher` at boot; it refreshes immediately and every 10 minutes. The catalogue is in `AppState::models`, and `ModelCatalogue::window` answers a model's context window for the proxy's usage meter.
