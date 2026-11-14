# Prompt cache readings and the compression loop

## What the numbers mean
`UsageRecord` (`crates/api/src/usage.rs`) is built from `usage` events: `inputTokens` is the whole prompt, `freshInputTokens = input - cached - creation`, `hitPercent = cached / prompt * 100` (null for an empty prompt). Per wire (`crates/proxy/src/meter.rs`): Anthropic adds `cache_read_input_tokens` and `cache_creation_input_tokens` to `input_tokens`; OpenAI totals already include the cached part; Gemini reports `cachedContentTokenCount` and no creation count.

Reading a low hit across rows: a stable head of a few thousand tokens is the system prompt plus tool definitions, and it hits even when the conversation body is new. Rows that keep only that head, mixed with rows near 100% and rows with a cold 0%, mean the body prefix is being rewritten between requests — not a broken meter.

## The compression loop
A fold replaces the folded turns with a freshly written summary turn, so every fold changes the prompt prefix and throws the body cache away. `crates/proxy/AGENTS.md` (Nudge gate) and `crates/proxy/src/nudge.rs` hold the rules:

- The first nudge fires at `min(75000, window / 2)` while the gate is open and compressible mass is at least `MIN_COMPRESS_RANGE` (5000). The 75000 floor is absolute, so on a 1M-token window it fires at seven percent of capacity.
- A compress clears only the tier flags: the first-nudge gate stays shut, so the next fold waits for `NUDGE_GROWTH_TOKENS` (50000) of new context, measured from the post-fold size because `last_nudge_tokens` follows a shrunk context down.
- One fold costs two cold requests, not one: the request carrying the new summary misses everything (0%), the next one hits only the unchanged head of a few thousand tokens, and the request after that is back near full.

Symptom in the events: a `compression` `action: "nudge"`, `kind: "first"` with a `sinceLast` far under `NUDGE_GROWTH_TOKENS` immediately before every `action: "compress"`, and a cold usage row right after each fold. That shape means the gate re-opened after a fold: an old build, or a regression in `reset_after_compress` and its callers.

A fold at a small fraction of the window is worth questioning even when the gate behaved. When a later stretch of the same Session sits at a larger context with a near-100% hit rate, the earlier folds bought nothing.

## Quantifying the churn
A cold row is a `usage` event with `cachedTokens == 0` and more than 20000 `inputTokens`. Sum their `inputTokens` and divide by the Session's total input: that share, not the hit percentage, is what the churn cost. Repeat over the neighbouring Sessions to separate a single-Session defect from a system-wide policy. Per-Session counts of `action: "compress"` and `action: "nudge"` belong in the same table, and a `fold-miss` action points at a block whose stored turns no longer match.

## Classifying cold rows without a fold
Bucket every cold row (`hitPercent` under 30) by the first cause that fits, and report the counts:
1. Input dropped sharply versus the previous row: a fold (`compress`, or `fold-miss`).
2. Gap since the previous `usage` row over 5 minutes: provider cache TTL.
3. Row is the first request after a `prompt` event (under 30 s): head-only hit (about 15646 cached = system plus tools). This is a correlation in long Gemini Sessions, not a proven cause: short fresh repro Sessions (18k and 69k context, prompts 1-185 s apart) stayed 86-97% warm on every new prompt, so a new prompt alone does not break the cache.
4. Head-only row with a gap under 20 s and no fold: TTL is ruled out. Tabulate which events fall between the previous `usage` row and this one (`nudge`, `steer_delivered`, `prompt`, `error`). In long Sessions, requests carrying a `steer_delivered` (sub-agent report) were head-only in roughly half the cases (about 6 of 11), so test steers first. Cold after a nudge was a weak signal (about 4 of 29).
5. Otherwise unexplained: possible provider-side eviction. Settle it by diffing `EMBERLOOP_PROXY_DUMP` bodies of the last warm row and the cold row. In a repro, folded rows (0%) and first-after-prompt rows had a byte-identical head (`systemInstruction`, `tools`, `toolConfig`, `generationConfig`, `sessionId`, `contents[0]`) and, for the prompt row, identical earlier contents, so the proxy-built body is not the cause there. A fold changes only the summary turn, yet the head also reads 0 cached: treat a fold row as fully cold on Gemini.
6. Still unexplained after identical bodies: diff HTTP headers. Run a logging reverse proxy (stdlib `http.server` + `http.client.HTTPSConnection`, chunked pass-through, log headers/body sha per request, never print `authorization`) on a local port and start the repro daemon with `--upstream http://127.0.0.1:<port>`. Headers on the agy/Google wire were identical across warm requests (only `content-length` varied), so headers are an unlikely cause. Write the forwarder from `execute` (the `write` tool refuses paths outside the project root) and run it with `bash` `background: true`.

`nudge` and `skill-review` events do not themselves change the prompt. The `skill-review` event is published after the fork finishes, so it is a marker, not a cause. Whether a delivered steer rewrites an earlier turn is open: head-only rows after steers say check it, so read the splice code before claiming a steer only appends at the tail.

## Reproducing and monitoring
- A live cache question is best answered by watching a real Session: poll `/api/sessions/<id>/history?limit=80` every 8 s from `execute`, dedupe by `seq`, and print `usage` (input, hit %, gap since previous usage) interleaved with `compression`, `prompt`, `steer_delivered`, `error`. Cold rows then show next to the event that preceded them.
- A synthetic repro needs the real shape: context over 100k, many tool calls, sub-agent steers. Toy Sessions (small context, `ls`-style prompts) stay warm and prove nothing. A test Session on the live daemon is `POST /api/sessions` with `projectId`, `model`, `thoughtLevel`, then `POST /api/sessions/<id>/prompt`. `EMBERLOOP_PROXY_DUMP` cannot be switched on in a running daemon; diffing request bodies needs a second daemon with its own `--port`, `--data-root` and `--config-root` (start it with `bash` `background: true`; a trailing `&` or `nohup` is refused). Dumps are `<session>.out.NNN.json`; usage row i is dump `2*i-1`. A repro daemon can resume an existing Session with `POST /prompt`, no new Session needed. Idle gaps are not a fixed TTL: gaps of 126-183 s went cold and a 203 s gap stayed warm.

## Background skill review
The review fork (`run_review` in `crates/proxy/src/server.rs`) logs each round to `<session>/review.jsonl` (`at` in seconds, `input`, `cached`). Its rounds are not `usage` events, so they never show in the cache page. Normal rounds hit 90-99%. To test "the review invalidates the main cache", compare each review run with the first main row after it and with the `prompt` events. Do not conclude from adjacency alone: reviews fire at turn end, so the next main row is often a new user prompt. A review round 1 that is itself cold (cached equals the head only) means the main cache was already cold before the fork, so the fork is not the cause.

## Knobs
`proxy_compression` is the master switch, `inject_nudge` removes the nudge text alone, `max_context_limit` sets the pressure band. `nudge.json` in the Session directory holds the gate state. `EMBERLOOP_PROXY_DUMP` writes the per-request bodies so two consecutive prompts can be diffed when the numbers alone do not explain a miss.
