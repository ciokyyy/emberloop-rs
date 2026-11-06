# AGENTS.md — crates/core

## Purpose

Defines shared workspace foundational types:
- `Context`: scoped services registry and cancellation token tree
- `EventBus`: typed event publish/subscribe
- `id`: unique identifier generation
- `providers`: the API-key provider registry (id, name, pi's default `baseUrl`) shared by the proxy route, the pi harness and the Providers API

## Steering

`SteerQueue` persists entries to the Session's `steer_queue.json` and reloads
in-flight entries as pending. An entry is delivered when the upstream returns a
2xx response head; a non-2xx or a connection failure before the head requeues it.
`begin_request`/`end_request` hold a per-Session in-flight flag, so `wake_if_idle`
fires a waker only when no request is in flight and the queue is not empty. One
wake drains the whole queue as a single `"\n\n"`-joined prompt.

## Child DOX Index

(none)
