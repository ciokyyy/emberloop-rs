# Proxy fault fixtures and dropped-body wording

How to reproduce and classify a provider connection that breaks mid-reply, for `crates/proxy`.

## A truncated body needs a raw socket, not an axum fake

An axum/hyper fake cannot truncate a response body:

- Declaring `content-length: 4096` and streaming fewer bytes makes hyper panic server-side (`payload claims content-length of 92, custom content-length header claims 4096`), and the client sees `kind: Request ... IncompleteMessage` at `send()`.
- A body stream that returns `Err` after one chunk looks right server-side but the client surfaces it as a request error, not the production failure.

Serve it from a bare `tokio::net::TcpListener` instead: read the request (headers up to `\r\n\r\n`, then `content-length` bytes), write the head with `transfer-encoding: chunked`, one chunk, then close the socket without the terminating `0\r\n\r\n`. That is what a dropped provider connection looks like, and reqwest reports the exact production text: `error decoding response body`, `err.is_decode()`, `err.is_body()`.

Answer later calls on the same listener with a normal `content-length` reply so one test asserts red-then-green: the first request fails mid-body, the retry succeeds. Keep the fake axum upstream as the default and add a fixture variant that points `ProxyConfig::with_upstream` at the raw socket's `http://127.0.0.1:<port>`.

## What the proxy must do with it

- A generated answer read with `response.bytes()` (non-stream) may be replayed: nothing reached the harness, so re-send the request and read again, up to the shared retry count, with the injected backoff duration (`Duration::ZERO` in tests).
- A URL that does not use reqwest auto-decompression cannot be blamed on gzip/br: the reqwest feature list has no `gzip`/`brotli`/`zstd`/`deflate`, and `is_hop_by_hop` strips the client's `accept-encoding`. The body really was truncated upstream.
- Every message for a decode or body failure must carry the same `stream was interrupted: ` prefix on the wire **and** in the published `EventKind::Error` data: `crates/api/src/subagent_recovery.rs` classifies that event, and an unprefixed decode error reads as hard and kills the worker. Word the event with `error_text(&error)`, never `format!("{error}")`.

## Harness retry banners

The `Retrying (attempt n/3, waiting 2s)...` / `Retry finished, resuming.` lines are produced by the pi harness, not by this repo, so the proxy never sees them. They arrive as whole-message ACP assistant chunks and must be detected in the daemon's `HarnessEvent::AssistantChunk` mapping (`crates/daemon/src/runner.rs`, `map_event`), which publishes `EventKind::Notice` with `route: "retry"` and `state: retrying|finished` instead of prose. Grep a recorded Session (`/root/.local/share/emberloop-rs/sessions/<id>/events/*.jsonl`, `"kind":"assistant_chunk"`) to confirm each banner is its own event before matching on the exact string.
