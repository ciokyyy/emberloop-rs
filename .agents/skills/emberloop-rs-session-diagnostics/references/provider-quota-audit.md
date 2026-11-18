# Provider quota and model-tier audit

Answer "how much usage do I actually have, and which model goes in which tier" from live numbers, not vendor marketing.

## Procedure
1. Fleet token facts from `GET /api/status/tokens`. Rows are per model × harness × project, so group by `provider/model` before ranking: share of `sum(totalTokens)`, cache hit as `cachedTokens/inputTokens`, mean latency as `totalLatencyMs/calls`. Print the top dozen groups and the grand total; the tail is noise.
2. Subscription windows from `GET /api/providers/<provider>/usage` for every subscribed provider. `windows[]` is `{label, usedPercent, resetsAt}`; `resetsAt` may be ISO with a `Z` or bare epoch seconds. These windows are per plan, so never present one as a per-model allowance.
3. Vendor allowances from the plan docs, for the model list and per-model caps:
   - opencode.ai/docs/console/go — limits are monthly dollar amounts per model; each model's 5-hour window is 20%, weekly 50%, monthly 100% of it; free models are unlimited; Extra Usage falls back to console credit.
   - support.claude.com usage-limit articles and claude.com/pricing — rolling 5-hour plus a weekly limit on one shared pool across web, desktop, mobile and Claude Code; the premium model family is credits-only except during a launch promo.
   - antigravity.google/docs/plans — baseline Gemini quota on every plan, third-party models only on the top plan; overage through AI credits at Gemini Enterprise prices.
   - chatgpt.com/pricing and help.openai.com — text chat unlimited under guardrails, every real cap sits on tools and agents, and few numbers are published.
4. Infer the account tier from what it can reach, not from its name: the model catalogue (`/api/models`, or `/root/.local/share/emberloop-rs/models/catalogue.json`) lists only reachable models, so third-party models under `agy` mean the top plan.
5. Deliverable: one line per provider (window percentages with reset times, token share, what spending it costs), then the ladders. Ladder shape: smol = cheap model at low effort with a same-provider fallback; mid = mid model at high effort that escalates on error; top = frontier model at high or xhigh with one cross-provider fallback for availability. Put a subscription whose window is near its limit lowest in the ladders, and the barely-used one highest.

## Pitfalls
- Allowances change fast (promos that loan a premium model expire), so re-read the doc page before recommending; the live `usedPercent` is the only figure worth quoting as exact.
- Where the vendor publishes no number (Codex on the cheap chat plan, Antigravity per-model quotas, Claude's weekly size), say "not published" and report the live percentage. Never invent a message count.
- Totals are input-token dominated with a high cache hit, so a token share is not a quota share; state both or the recommendation reads as fact when it is a guess.
- A 5-hour window back at 0% while the monthly window stays high means the rolling window reset, not that the allowance returned.
- Reasoning effort is part of the tier: the same model at `low` and `xhigh` belongs on different rungs, so ladders name both.
