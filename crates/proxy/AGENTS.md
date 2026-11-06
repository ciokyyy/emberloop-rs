# AGENTS.md — crates/proxy

## Purpose

Model-wire proxy for every harness.
- `router` serves `/health` and `/p/:token/*rest`; `TokenSigner` verifies `hex(session_id).hex(HMAC-SHA256(secret, session_id))`, so a path only reaches the Session it was signed for.
- `handle_generation` parses Cloud Code requests (`request` object or root), injects the pinned policy and Ponytail text into `systemInstruction`, and appends missing `compress`/`decompress`/`search_context` declarations to `tools`.
- SSE responses stream through. A proxy `functionCall` part is withheld and the tool runs locally; a pure proxy turn issues a follow-up request into the same client response, a mixed turn keeps only the harness calls and stores the proxy pair as a splice. `MAX_TOOL_ROUNDS` is 8, each round has `ROUND_TIMEOUT`, and a dropped client body drops the upstream stream.
- `execute_tool` runs `compress`, `decompress` and `search_context` on the conversation; the harness never sees a proxy call.
- `SpliceStore` persists to `<data root>/sessions/<id>/splices.jsonl`, append-only, keyed only by the tool-result turn id. `Splice::marker` is the exact string that proves a splice is already applied, so double-apply is impossible; a missing harness anchor drops the splice, a self-anchored proxy pair attaches before the newest turn. Opaque fields and thought signatures stay byte-exact.
- `meter::extract_usage` reads `usageMetadata`/OpenAI `usage` from JSON or the last SSE payload; usage is published once per completed generation.
- `Compressor` trait: `compress`, `decompress`, `search_context`, `status`, plus `apply` to refold a harness-rebuilt history. `NativeCompressor` swaps a turn range for a summary marker and spills originals to `<session spill>/blocks/<id>.json`.

## Tests

`src/tests.rs` runs a fake upstream. Covers token tamper, prompt/tool injection, usage, byte-exact splice replay after reload, dropped-anchor and double-apply rules, SSE proxy-only continuation, mixed-turn rewriting, the 8-round cap, client disconnect, and round timeout.

## Child DOX Index

(none)
