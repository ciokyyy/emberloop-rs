# Ticket 20: Billion-context parity (nudges, status, rules, backstops) with golden fixtures

## Goal

The proxy's native compressor behaves like billion-context for everything the model sees and everything that decides when it sees it, proved by golden fixtures generated from the reference, not by reading its docs. Reopens the phase 1 line "no tiers, nudges or automatic compression" (`specs/phase-1/spec.md`, the Billion context bullet); that line is replaced by this ticket when it lands. Blocked by ticket 08, 09. Version checks (ticket 21) tell the User when the reference moved.

## Status

Done. P0 is built: nudge gate (first, pressure, emergency, tier 2/3), `acp_status`, protected tools, `preserveRecentMessages`, preflight backstop, emergency truncation, marker line and anti-forgery, `injectTool`/`injectNudge`, and the Settings -> Context panel. Golden nudge decisions and status numbers are tested; tier 2/3 remain not golden-pinned because no reference scenario reaches 1000/2000 blocks. `acp_rule`, output headroom and the rest of the out/P1 list are unchanged.

## Reference

`billion-context` (npm, MIT). Baseline: **0.1.212, commit daa082ea**, recorded in `crates/proxy/parity/BASELINE` together with its kernel, **acp-kernel 0.0.178** (the `kernel/` directory at that commit). The standalone npm `acp-kernel` stops at 0.0.101 and lacks the reference's pressure, emergency and growth-step logic, so it is not the reference. `crates/proxy/parity/regen.sh` clones the repo, checks out the baseline commit, builds `kernel/` with `tsc` (no install scripts) and runs `regen.mjs` against it. It publishes almost daily, so ticket 21 watches the baseline. Everything below is read from its `CONFIGURATION.md` and kernel source, never from the README.

## Parity matrix

P0 is this ticket. P1 is a follow-up the User picks. Out is not planned.

| Behaviour (bili setting) | Reference default | Level |
|---|---|---|
| `compress`, `decompress`, `search_context` semantics, protected tools, `preserveRecentMessages`, all-protected range is an error | kernel | P0 (compress/decompress/search exist; protected tools and recent-message rule are new) |
| `acp_status`: context usage and which ranges are still compressible | kernel `status()` | P0 |
| First nudge: fires when context tokens reach `min(FIRST_NUDGE_TOKENS, 0.5 x window)`, with `FIRST_NUDGE_TOKENS` = 75000, so any model with a window under 150000 gets it at half its window. Needs compressible mass, like the reference's T1. It repeats every `nudgeGrowthTokens` (50000) of context growth until a fold lands | 75000; 50000 growth | P0 |
| Pressure nudge: `maxContextLimit`; every turn while over; `minPressureBenefitTokens` floor. Only after the first nudge has fired | 75%; `max(5000, limit x 1%)` | P0 |
| Emergency: `emergencyThresholdPercent`, forced nudge plus truncation of large visible tool outputs, never summaries. Only after the first nudge has fired | 95% | P0 |
| Baseline reset after a successful compress; first-sight mass bypass; repeat nudges need positive growth | kernel | P0 |
| Tiers T2/T3 (`tiers.tier2Trigger`/`tier3Trigger`) and tier-2/3 distillation nudges. Only after the first nudge has fired. Their token-mass triggers use the reference's T2/T3 values (step x 1.5 of 50000) | counts 1000/2000 | P0 |
| Marker line and anti-forgery: the 📦/❌ marker after a proxy tool, stripping model-forged markers (`#717`), marker-integrity note | on | P0 |
| `injectNudge` and `injectTool` switches | true | P0 (Settings) |
| Preflight backstop: input alone over the window is folded before forwarding | kernel | P0 |
| `acp_rule`: persistent principle-level reminders, hard-protected from compression, re-injected into the system prompt every turn | opt-in tool | Out for now (User decision 2026-11-06); revisit after the nudges ship |
| Output headroom reservation (`outputReservePct`) so the nudge bands sit below `window - reserved` | 0.25 | P1 |
| `nudgeModelDecided` side call, per-model and per-provider overrides | off | Out (decided by the User 2026-11-06) |
| `autoFold` external chain, `crush`, `absorb`, `acp_retrieve` and its export pointer, `reasoningGuard`, prompt packs, `/acp-cache` ledger, provider and model overrides | off or host-specific | Out; CCR's arrival replacement is built on our own store, see the decision below |

Settings keys live in Settings -> Context and use the reference names where the behaviour is the reference's, so a User moving from bili reads the same words. There are no per-model overrides: one global set, and the only window-dependent number is the first nudge below.

**The first-nudge rule (User decision, 2026-11-06).** Until a Session has had its first nudge, none of the later nudges (pressure, emergency, tiers) can fire; the first nudge is the gate. The first nudge fires when context tokens reach `min(75000, 0.5 x window)` (window from the catalogue, as the meter already reads it). A successful compress resets the Session to "no nudge yet" with the reference's baseline reset, so the cycle repeats.

**The nudge repeats on growth (User decision, 2026-11-06, after a live Session ran to 300000 tokens on one ignored nudge).** A single first nudge assumes the model obeys it; when it does not, nothing fires again until 75% of the window, which on a 1M window is 750000. The reference's repeating `nudgeGrowthTokens` step (50000) is therefore used: after any nudge, once the context has grown by 50000 tokens and compressible mass is still there, the first-nudge text rides the next request again. The step is a constant for now, not a Settings key.

## How parity is proved

1. **Spike (throwaway):** drive `acp-kernel` from the installed package with a small Node script over recorded conversations (a short chat, a long tool-heavy run, a run past 75%, a run past 95%, a run with a protected tool, a run with 1000+ blocks). For each, record per request: input conversation, nudge text or none, `acp_status` text, truncation decisions. These become files in `crates/proxy/parity/golden/` with the baseline version in each file name.
2. **Tests first:** a Rust test per golden file feeds the same conversation to `NativeCompressor` plus the new nudge engine and asserts the same nudge decision, the same status numbers and the same marker text, byte for byte where the reference text is fixed (the nudge wording is copied from the reference with attribution in the file header; MIT notice kept) and within an explicit tolerance only for token counts (the reference estimates tokens; the tolerance is one test constant, not per test).
3. The Node script is not part of the check and is not a runtime dependency. `parity/regen.sh` rewrites the golden files from the commit in `BASELINE`; to review a newer upstream, a reviewer changes `commit` in `BASELINE`, reruns it and reads the diff of `golden/`.

## Findings from the golden run (2026-11-06, kernel 0.0.178)

- Pressure first fires when usage reaches 75 percent of the window ("OVER-LIMIT T1") and then on every turn while over; emergency starts at 95 percent ("EMERGENCY T1"). A protected tool produces no nudge.
- The generator feeds the kernel the token count of the pruned history, as a host does; feeding the raw history made a compress invisible and the reset scenarios meaningless.
- Reason strings carry the reference's own numbers (including its 50000 growth gate), so tests compare the decision (inject or not, tier, over-limit and emergency flags, status numbers), never the reason string, except for the fixed nudge text.
- Token counts in the goldens are `ceil(chars / 4)` per message; the Rust side reproduces them exactly.
- Tier-2/3 distillation could not be reached by any scenario (the reference needs 1000 and 2000 active blocks; pressure mode keeps T1). Those paths are implemented from the kernel source with unit tests and are marked "not golden-pinned" here until a scenario reaches them.
- Nudge text is rendered with the kernel's default prompts and no lane-specific sections or protocol; lane wording is not pinned.
- Naming: the kernel key is `emergencyThresholdPct` (0.95), the host key is `emergencyThresholdPercent` ("95%"). Settings use the host name.

## Decisions taken during implementation (2026-11-06)

- **Compress protocol stays ours.** The reference's nudge text speaks of message refs (`m00002–m00125`) and one multi-block string; our `compress` takes numeric `start`/`end` turn indices. The nudge wording is copied from the reference, with the protocol paragraph and the range labels rewritten for turn indices. Batching is kept: one `compress` call takes `ranges`, a list of `{start, end, summary, topic?}` (the single form still works). This is the one deliberate departure from verbatim text; the gating, bands and numbers are pinned by the goldens. Ref-based compress (rendering `<acp>` refs into messages) is a separate ticket if wanted.
- **One body for pressure and emergency.** The reference's static wording is identical; only the breakdown and range lines differ, so `nudge.pressure` and `nudge.emergency` share a body with those lines as placeholders. `nudge.first` is the first-nudge text.
- **Raw templates are pinned, rendered per request.** `PinnedPrompts::pin` renders with fixed variables, which would bake the dynamic lines (breakdown, since-last, ranges) into empty strings. The nudge ids pin the raw template, and the proxy renders it per request with a public render helper. The rendered nudge is transient: it is put into the outgoing request right after the newest tool result and no splice stores it, so a later request carries it only if the gate fires again.
- **`acp_rule` is not built.**
- **Arrival replacement is ours, on our store (User decision 2026-11-06).** The reference's CCR is the model-free half of its default: an oversized tool result is stored once at arrival and its visible text becomes a deterministic placeholder, so the bytes never change again and the prefix cache survives. Ours lands in `crates/proxy/src/ctx.rs`, points at a `ctx:` handle that `read ctx:<handle>` and `ctx_search` already serve, and keeps no `acp_retrieve`: the store and its retrieval tools exist, so the reference's tool is not rebuilt. Cache accounting ships with it as `cachedTokens` on the usage event, so a fold's cache cost is measured instead of assumed.

## Design

- `crates/proxy/src/nudge.rs`: a pure state machine `NudgeState` with `observe(usage, compressible_mass, blocks) -> Option<Nudge>` and `reset_after_compress()`. No clock, no IO. State persists in the Session directory next to `splices.jsonl` (atomic rename), so a restart does not re-fire a nudge.
- Where it runs: `prepare_request` asks the engine after the splice and block apply and, when a nudge fires, puts it as a text user turn right after the newest tool result of the outgoing request. It is transient (no splice), and it runs last, on the final contents after splices, so its turn numbers match compress. The harness never composes it. The text comes from `crates/prompts` as a new prompt id (`nudge.growth`, `nudge.pressure`, `nudge.emergency`, `nudge.tier2`, `nudge.tier3`, `acp_status.description`), overridable like every prompt.
- `acp_status` is a proxy-only tool like `compress`: declared by the proxy, executed in the proxy, never shown to the harness.
- Emergency truncation edits a copy of the outgoing request only; the stored history is untouched and the truncated tool output stays in `spill/` with its handle.
- The usage numbers come from the existing meter and the catalogue window; no second source.
- The event log gets a `nudge` payload on the existing `compression` event kind (`{action: "nudge", tier, reason}`) so the feed can show why the model was asked.

## Tests first

Failing before implementation: each golden file; the first nudge fires at 75000 for a 1M window and for a 200000 window, at exactly 75000 for a 150000 window, and at 0.5 x window for a 128000 and a 64000 window; no pressure, emergency or tier nudge fires before the first one even when usage is over their thresholds; after a compress the gate closes again; pressure and emergency nudges fire every turn while usage stays over their band, as the reference does (no growth requirement); a compress closes the first-nudge gate again; a restart in the middle keeps the baseline; pressure nudges fire every turn while over and stop when under; emergency truncation never touches a summary and leaves the stored history intact; a forged marker in model output is stripped; `injectNudge=false` sends none but the meter still tracks; `acp_status` text matches the golden; a transient nudge is absent from the next request unless the gate fires again, and no splice file records it; the whole set stays inside the loop-check budget with no real sleep.

## Out of scope

Running bili itself (spec: one proxy), model-decided nudges, per-model overrides, auto-fold, CCR, prompt packs.

## Docs to update in the same change

`crates/proxy/AGENTS.md`, `crates/prompts/AGENTS.md` (new ids), `web/AGENTS.md` (the `compression` event payload and Settings -> Context), `specs/phase-1/spec.md` (the Billion context bullet), `specs/phase-1/issues/AGENTS.md`.
