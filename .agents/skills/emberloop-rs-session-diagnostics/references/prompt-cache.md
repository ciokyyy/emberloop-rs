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

## Knobs
`proxy_compression` is the master switch, `inject_nudge` removes the nudge text alone, `max_context_limit` sets the pressure band. `nudge.json` in the Session directory holds the gate state. `EMBERLOOP_PROXY_DUMP` writes the per-request bodies so two consecutive prompts can be diffed when the numbers alone do not explain a miss.
