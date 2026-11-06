# Ticket 02: Workspace, check script, Context tree, stores, event log

## Goal

Create Cargo workspace (`core`, `store`, `api`, `harness`, `harness-agy`, `proxy`, `tools`, `web`, `daemon`), `scripts/check` (fmt, clippy with cognitive complexity, tests), Context tree, data root, Projects and Sessions store, and append-only event log.

## Status

Completed. All tests pass and `scripts/check` runs cleanly.

## Deliverables

1. **Workspace**: Cargo workspace setup with `crates/core` (`emberloop-core`) and `crates/store`.
2. **`scripts/check`**: Verifies `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings -W clippy::cognitive_complexity`, and `cargo test --workspace`. Passes cleanly.
3. **Context Tree (`crates/core/src/context.rs`)**: Type-based service container with hierarchical `CancellationToken` support.
4. **Event Bus (`crates/core/src/events.rs`)**: Typed `SessionEvent` and broadcast channel bus.
5. **DataRoot & Stores (`crates/store`)**:
   - `DataRoot`: manages layout `<data_root>/projects.json`, `<data_root>/sessions/<id>/{meta.json, events.jsonl, images/, spill/}`.
   - `ProjectsStore`: atomic JSON CRUD for Projects.
   - `SessionsStore`: atomic JSON CRUD for Session metadata.
   - `EventLog`: append-only JSONL log with sequence number generation and `replay(after)`.
