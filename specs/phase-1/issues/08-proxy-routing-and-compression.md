# Ticket 08: Proxy: routing, meter, Ponytail, splices, compression

## Goal

Model proxy on `/p/<session-token>/...`, usage metering from last request tokens, Ponytail system prompt injection, stored splices, billion context through the `Compressor` trait with a native Rust backend (`compress`, `decompress`, `search_context`, `status`), originals kept in `spill/`, no bili process, no automatic compression. Blocked by tickets 01, 03, 12.

## Status

Done. Token routing, meter, Ponytail, stored splices and the native `Compressor` are tested in `crates/proxy/src/tests.rs` against a fake SSE upstream. Steer injection belongs to ticket 09.

## Tests first

Failing before implementation, with message-shape fixtures from the Go repo's bili tests and probe scripts: compress replaces a range with one block; decompress restores the exact originals; search_context finds text only in compressed originals; the proxy never shows the harness a compress call; a compressed Session replays byte-identically on the next request; the trait is satisfied by a fake backend (proves the seam for a later bili backend).

Also covers, from decision M: splices persisted in `splices.jsonl` keyed by the tool-result turn id only; the SSE streaming path (withhold a proxy `functionCall`, run it, follow-up request, continue streaming; only-proxy loop capped at 8; a mixed turn rewritten to harness calls only; client disconnect cancels upstream and tool; per-round-trip timeout), each with a fake SSE upstream test; a constant-time token compare with a per-start key.
