# AGENTS.md — crates/core

## Reviewer subagents

`steer::is_reviewer_role` identifies reviewer roles (`spec`, `standards`); `steer::SubagentReport` carries subagent identity (`session_id`, `role`, optional `ticket_path`) and report metadata for every subagent role, omitting empty `ticket_path` from `enqueue_report`'s parent steer header. `enqueue_report` persists each distinct submission before acceptance and rolls back failed writes; queue mutations save under the entries lock. Direct and factory queue loading both requeue in-flight entries. `tests/reviewer_reports.rs` covers repeated reports, restart and storage failure.


## Purpose

Defines shared workspace foundational types:
- `Context`: scoped services registry and cancellation token tree
- `EventBus`: typed event publish/subscribe. It counts unnumbered (seq 0) events a persister has not stored yet; `settled(limit)` waits for that backlog so the runner numbers its own events after the proxy's thinking and usage (`persister_started`, `event_stored`, `events_lost` are the persister's side). Without a persister it never waits.
- `id`: unique identifier generation
- `providers`: the endpoint registry (`ProviderEndpoint`: id, name, pi’s default `baseUrl`). `KEY_PROVIDERS` / `key_provider` feed the key UI; `OAUTH_PROVIDERS` adds Codex at `https://chatgpt.com/backend-api`. `proxy_provider` resolves both kinds for signed proxy routing. The pi harness uses the same descriptors; OAuth endpoints never appear as API-key providers and the registry contains no credentials

## Steering

`SteerQueue` persists entries to the Session's `steer_queue.json` and reloads
in-flight entries as pending. An entry is delivered when the upstream returns a
2xx response head; a non-2xx or a connection failure before the head requeues it.
`begin_request`/`end_request` hold a per-Session in-flight flag, so `wake_if_idle`
fires a waker only when no request is in flight and the queue is not empty. One
wake drains the whole queue as a single `"\n\n"`-joined prompt, with every queued image attached in order.
`set_turn_active` is async and returns the entries a turn end releases: it seals
every open batch, and a sealed batch with no pending member flips its `Held`
reports to `Pending` behind one `Batch "<name>" completed (n/n):` notice.
`SteerEntry.batch` names the barrier; `register_batch_member` rejects a sealed
batch or a child already pending in one, `unregister_batch_member` drops an
empty batch or releases a sealed one, and `complete_batch_member` moves a
finished child to `completed_ids`. The queue persists `{ entries, batches }`
while any batch is open and a bare array otherwise; reload preserves `Held` and
requeues only `InFlight` as `Pending`. `SessionEvent::steer_queued` builds the
one `SteerQueued` payload for a plain steer and a Subagent report.
`expire_batches(now)` closes every batch past `deadline_at`: it prepends a
timeout notice, adds a synthetic `timed_out` report for each pending member and
releases the batch. `reconcile_on_startup` seals pre-restart batches, closes
orphaned pending members as `errored` reports when no turn is active, and
returns the released entries.

## Child DOX Index

(none)

## Title events

`EventKind::SessionTitle` serializes as `session_title` with `{ "title": "..." }` data after metadata persistence. It changes neither turn status nor usage.
