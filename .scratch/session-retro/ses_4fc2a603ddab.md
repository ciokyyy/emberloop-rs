# Compression retrospective across ten sessions

## Scope and evidence

The user reported that the cache view for `ses_635f77c61d57` showed the prompt cache invalidating on every request, and asked whether the invalidation was real or a display error. This audit covers that session plus the nine before it, and the question asked here is narrower than the earlier session retro: how did the compression system behave.

The primary sources are the recorded event logs under `/root/.local/share/emberloop-rs/sessions/<id>/events/*.jsonl` for the ten sessions `ses_4fc2a603ddab`, `ses_101061d11451`, `ses_635f77c61d57`, `ses_378ffc138fbc`, `ses_c65c3a5f4935`, `ses_a727101e4e36`, `ses_3b8e15ad67d5`, `ses_71499ad5e9f2`, `ses_509a846aeead` and `ses_db036c570374`, cross-checked against the current Rust source. `usage` events carry `inputTokens`, `cachedTokens`, `cacheCreationTokens`, a per-role `breakdown` and `contextWindow`; `compression` events carry the action (`nudge`, `compress`, `preflight`, `fold-miss`, `skill-review`), the nudge kind and sizes, and the fold ranges. A `cold` row is a `usage` row with `cachedTokens == 0` and more than 20,000 input tokens: a request that paid full price for a context the previous request had already read.

## The invalidation was real, and it was one session

| Session | Requests | Folds | Nudges | Input tokens | Cached | Cold rows | Cold tokens |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `ses_4fc2a603ddab` | 93 | 1 | 1 | 4.12 M | 95.3% | 0 | 0 |
| `ses_101061d11451` | 351 | 3 | 7 | 29.85 M | 98.1% | 1 | 0.05 M |
| `ses_635f77c61d57` | 1385 | 40 | 47 | 98.60 M | 90.5% | 39 | 2.61 M |
| `ses_378ffc138fbc` | 374 | 17 | 19 | 32.26 M | 93.0% | 0 | 0 |
| `ses_c65c3a5f4935` | 93 | 2 | 3 | 5.80 M | 89.3% | 3 | 0.19 M |
| `ses_a727101e4e36` | 766 | 30 | 35 | 86.64 M | 94.3% | 3 | 0.16 M |
| `ses_3b8e15ad67d5` | 51 | 2 | 2 | 2.35 M | 88.2% | 1 | 0.03 M |
| `ses_71499ad5e9f2` | 68 | 1 | 1 | 2.86 M | 95.1% | 0 | 0 |
| `ses_509a846aeead` | 234 | 7 | 8 | 13.99 M | 95.0% | 0 | 0 |
| `ses_db036c570374` | 23 | 0 | 1 | 1.16 M | 93.8% | 0 | 0 |
| Total | 3438 | 103 | 124 | 277.63 M | 93.1% | 47 | 3.02 M |

Cold tokens are 1.1% of all input tokens, and 2.61 of those 3.02 million belong to the reported session. The other nine sessions paid 0.41 million cold tokens across 179.03 million, so compression is not expensive system-wide; it was expensive where the gate misfired.

In `ses_635f77c61d57` the recorded inputs show a repeating trace, quoted as `seq input/cached`:

```
5455 80277/77841 97%  5456 60229/0  5460 79686/8428 11%
5464 nudge first 79686 +19457   5465 compress [(1,90)]
5466 81792/79694 97%  5467 61953/0  5471 81412/8427 10%
5475 nudge 81412 +19459         5476 compress …
```

The pattern repeats through request 5540: a fold, a request that pays full price, a request that matches only the 8428-token head, then a step that grows the context back to the trigger and folds again. Two late nudges fired on almost no growth (`94541` with `sinceLast 1983`, then `95772` with `sinceLast 1231`); the model ignored the second one, the context then grew to 124,592 tokens, and every remaining request in the session hit the cache at 97–98%.

## Why it happened

`NudgeGate::first_threshold` is `min(75_000, window / 2)`, which is 75,000 on this session's 1,048,576-token window: seven percent of capacity. `reset_after_compress()` cleared `first_fired`, so the `!first_fired` branch reopened on every compress and the 50,000-token growth branch never ran. A fold leaves about 71,000 tokens, one step adds about 19,000, and the gate opened again. Because the fold splices a freshly written summary at the head of the conversation, the provider prefix changes, so the cache restarts from turn 1: 97% becomes 0%, then 11% once the unchanged head is re-cached. The fold was not merely useless, it was the cause of the invalidation it was reacting to.

The cache page reported this faithfully. `web/src/routes/p/[project]/s/[session]/cache/+page.svelte` is a one-line stub rendered through `web/src/routes/+layout.svelte:539`, and `web/src/lib/components/CacheView.svelte` renders `GET /api/sessions/{id}/usage` rows as Request, Cached, Written, Fresh and Hit. "Written 0" is also faithful: `crates/proxy/src/meter.rs` hardcodes `cache_creation_tokens: 0` for Gemini usage, which does not report it.

The fix is in `crates/proxy/src/nudge.rs`: a compress now clears only the tier flags, so after a fold the gate stays shut until `NUDGE_GROWTH_TOKENS` (50,000) of new context arrives. The guard beside it (`refuse_cache_costly_folds`, `crates/proxy/src/server.rs:1655`) compares freed tokens against the surviving tail, which these folds passed easily: the defect lived in fold frequency, not fold size.

## Candidates, most severe first

**1. The model is never told what a fold costs.** `crates/prompts/prompts/compress.md` describes the ranges and the summary contract and stops; `crates/prompts/prompts/nudge.first.md` gives the model `{{ since_last_nudge }}` and no consequence for acting on it. Searching for "cache" across `crates/prompts/prompts/*.md` matches only `tool.websearch.md`. The model therefore folds whenever asked, and the only brake is a gate it cannot see. This is the costliest gap because the fix above hardens one gate while leaving the model's decision rule untouched: a single sentence naming the cache restart and the growth needed before the next fold turns the model from a follower into a second brake. One sentence in each of the two files, so the payload cost is a nudge-sized token count.

**2. The cache view cannot show why a row missed.** The table lists hit percentage without marking the requests where a fold or a nudge landed, so a dip is unattributable from the UI, and the user had to ask an agent to explain what the page already knew. `GET /api/sessions/{id}/usage` returns the rows, and the `compression` events hold the fold positions, so marking the rows where a fold preceded the request is a join inside the API and a column in `CacheView.svelte`. Without it, every future cache question costs a session investigation.

**3. The first nudge threshold is absolute, so it fires at seven percent on a large window.** 75,000 tokens is a sensible trigger on a 200,000-token window and a very early one on 1,048,576. The evidence that this is premature rather than protective is in the same session: after the model ignored the final nudge, the context ran to 124,592 tokens with 97–98% cache hits, so the folds it performed at 75,000–95,000 tokens bought nothing. Scaling the first threshold with the window (for example `max(75_000, window / 10)`) restores the fold-at-capacity-pressure intent on large windows. This changes a parity constant that came from the billion-context goldens, so it needs a spec decision rather than a silent edit.

**4. No test replays a real usage trace.** The new tests in `crates/proxy/tests/nudge.rs` use four hand-picked observations from this session. The goldens that failed to catch the loop, `KNOWN_EARLY` and `FIRST_FIRES`, were derived from `billion-context` `daa082ea` and their early-nudge entries encoded the churn as expected behaviour. A fixture built from the recorded sequence (tokens, compress, tokens, compress, …) asserting a fold count per 50,000 tokens of growth would have failed on the old gate and now pins the fix to the recorded behaviour rather than to four numbers.

**5. A flaky test and two exceeded time budgets stand in the check.** `scripts/check` ended with `FAIL harness-pi::pi the_session_home_points_pi_at_the_proxy_and_the_tools`, which passes in isolation and in a full `cargo nextest run --workspace` (707 tests run, 707 passed, 8 skipped). The same run printed `budget exceeded: clippy 131391ms > 120000ms` and `budget exceeded: nextest 100714ms > 90000ms`. Per the repository's own standards a flaky test is a defect, and here it obscures a real failure: a red `scripts/check` that is probably load noise invites re-running until green. Either widen the spawn timeout in that test or quarantine it until fixed, and re-audit the budgets so a genuine regression in check time is visible.

**6. The fold guard counts size, not frequency.** `refuse_cache_costly_folds` refuses a fold that frees little relative to the tail it leaves. The gate now covers frequency, so no change is proposed; noting it here so that a future frequency problem is not looked for in the guard again.

Navigation cost nothing in this investigation: the causal file was found by grepping `reset_after_compress`, and the UI path was one stub, one shell line and one component. No steering file changed behaviour wrongly, and no instructions were found to be no-ops.

## Proposed changes

- `crates/prompts/prompts/compress.md` keeps its current text: it is a tool description and loads on every request, so the cache-cost line belongs in the nudge the model actually reads when it decides.
- A window-scaled first threshold, applied after asking: the parity constant moved, and the recorded Session makes the case. It sat at 124,592 tokens with 97–98% cache hits after the model ignored a nudge, so folds at 75,000–95,000 earned nothing.
- No fixture replay of the recorded 1,385-request trace: the two new tests pin the same arithmetic with four recorded observations, and a fixture asserting a fold count per growth step would restate them at the cost of a large committed data file.

## Applied changes

- `crates/proxy/src/nudge.rs`: `reset_after_compress` clears the tier flags only; a compress leaves the gate shut.
- `crates/proxy/tests/nudge.rs`: `after_a_compress_the_next_fold_waits_for_the_growth_step`, `a_compress_does_not_rearm_the_gate_on_the_next_step`, and the pruned `KNOWN_EARLY` parity table.
- `crates/proxy/src/server.rs`: `close_nudge_gate` documentation.
- `crates/proxy/AGENTS.md`: the nudge paragraph.
- `crates/prompts/prompts/nudge.first.md`: the cache-cost bullet under Compression Philosophy.
- `crates/prompts/AGENTS.md`: the cache-cost line recorded as new text under Attribution.
- `crates/api/src/usage.rs`: `UsageRecord.fold`, set by `mark_folds` from the `compress` events that fall between two requests, so a hit dip is attributable; `openapi.json` and `web/src/lib/api/schema.d.ts` regenerated.
- `crates/api/tests/usage.rs`: `a_request_after_a_fold_is_marked_and_the_requests_before_it_are_not`.
- `web/src/lib/components/CacheView.svelte`: a `fold` badge on the affected request and a footnote saying what it means; `CacheView.test.ts` covers it.
- `.config/nextest.toml`: the `pi` test binary, which spawns the harness binary, runs one test at a time with a 20s slow-timeout, so load no longer turns it into a spurious failure.
- `crates/proxy/src/nudge.rs`: `first_threshold` is now `max(min(75000, window / 2), window / 10)`, so a 1M window folds first at 104,857 tokens instead of 75,000 and a 200,000-token window is unchanged.
- `crates/proxy/tests/nudge.rs`: the gate cases gained the 1M window at 100,000, the growth tests moved to a 100,000-token start, and the `window-1m-steady` first fires moved to frame 20 at 100,505 tokens.
- `crates/proxy/src/tests.rs`: the first-nudge fixtures now carry 105,000 tokens (a 440,000-character result, or a stored usage reading), and the prefix-stability run drives 34 steps to reach the new gate.
