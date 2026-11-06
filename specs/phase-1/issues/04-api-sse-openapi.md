# Ticket 04: HTTP API, SSE, OpenAPI

## Goal

Axum HTTP API, SSE event streaming with `?after=<seq>` replay, utoipa OpenAPI generation and client generation. Blocked by ticket 02.

## Status

Done. Endpoints, `openapi.json` and the generated TypeScript client pass `scripts/check`. The SSE stream subscribes before the `?after` replay and drops live duplicates; `crates/api/tests/sse.rs` covers it. Not yet tested: `?after` across segments through the API, an unknown Session 404 body.

## Tests first

Failing before implementation: `?after=<seq>` replays across segment boundaries; the OpenAPI document lists every endpoint of decision G; the generated client is fresh (`git diff --exit-code`); an unknown Session id is a 404 with a body; SSE resumes after a dropped connection with no gap and no duplicate.
