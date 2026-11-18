# The Fallow health gate

`cd web && bunx fallow health --score --min-score 87.9` must pass before a web change is done, and it is a `scripts/check` stage. The score is `100 − sum(penalties)`, so a failing run is always one new deduction away from the baseline.

## Read the score, not the headline number

- Judge with `--score`, which is the gate. Plain `bunx fallow health` also charges `hotspots -10.0` and reports a much lower grade (`77 B`) with no change to the code; `--score` omits that penalty from its JSON too.
- The human `--score` line rounds. `Health score: 88 A` shows while the deductions sum to `100 − 12.2 = 87.8`, and the gate then fails with `Health score 87.8 (A) is below minimum threshold 88`. Never judge a gate run by the printed grade or worth: pass the floor (`--min-score 88`) and read the exit status.
- Which categories cost what: `bunx fallow health --format json` returns `health_score.penalties` (`dead_files`, `dead_exports`, `unit_size`, `coupling`, `complexity`, `hotspots`, …) plus the `vital_signs` those formulas read (`p95_fan_in`, `coupling_high_pct`, `critical_complexity_pct`, `functions_over_60_loc_per_k`, `dead_export_pct`). The full formula table is one page: <https://fallow.tools/docs/explanations/health/>.
- What exactly is dead: `bunx fallow dead-code --format json` returns `unused_files` / `unused_exports` with `path`, `line` and an `actions` list (delete, or suppress with a comment). `bunx fallow check` still works but is deprecated and prints a notice first.
- `bunx fallow dead-code` also prints advisory config notes (`entry pattern matched only files that are entry points anyway`, `ignorePatterns entry excluded no file`); those are pre-existing noise, not findings.

## Which penalty to attack

Every penalty is a rate times a weight, so one extra file or function moves it and a round of micro-edits usually cannot:

| Penalty | Formula |
|---|---|
| dead files, dead exports | `min(pct × 0.2, 15)` |
| complexity | `min(critical_complexity_pct × 4, 20)` — the critical cyclomatic tail only, not the 41 ordinary findings |
| unit size | `min(functions_over_60_loc_per_k × 0.5, 10)` — already maxed here |
| coupling | `min(coupling_high_pct × 0.5, 5)` |
| duplication | `min(max(0, duplication_pct − 5), 10)` |

`coupling_high_pct` is the share of analyzed files whose **fan-in** is above `max(p95_fan_in, 10)`. With 120 files that is 4 files → `3.3 → -1.7`, 3 files → `2.5 → -1.3`, and the floor of 10 means fan-in 10 sits under the line while 11 crosses it. A new leaf component that imports the shared row model (`import type { FeedItem } from '$lib/feed'`) is one edge and its mount test is another; that took `feed.ts` from 10 to 12 and cost `-0.4`. Fix the component, not the model: give the presentational row its own props (`{ text, retry }`) and pass the fields from `Feed.svelte`. Moving the shared type into a new module only relocates the fan-in (the type module lands near the same 10).

For the exact offenders read `file_scores[]` (`fan_in`, `fan_out` per file) and `findings[]` (`path`, `name`, `line`, `cyclomatic`, `cognitive`, `line_count`) from `fallow health --format json`, rather than guessing from the human summary.

## Find out which penalties were already there

A healthy tree scores about `88 A` against a `87.9` floor, with `unit size -10.0 · coupling -1.3 · complexity -0.4` already deducted: `unit size` charges the 60-LOC unit limit and the 600-line limit on `src/routes/+layout.svelte`, `StatusTokens.svelte`, `app.css` and the generated `src/lib/api/schema.d.ts`; `complexity` charges the `+layout.svelte` template (cyclomatic 75), the only function past the critical tail; and coupling charges the hub files (`api/index.ts`, `utils.ts`, `motion.ts`). None of those is yours to split for a ticket. Measure instead of guessing:

```bash
git worktree add /tmp/base <commit-before-your-change>^
cp -al "$PWD/web/node_modules" /tmp/base/web/node_modules
cd /tmp/base/web && bunx fallow health --score --format json
```

Compare the `penalties` object with the current run. Deductions the baseline already had are debt; the delta is what you fix. Remove the worktree afterwards.

A directory outside `web/` resolves some imports differently — `web/tsconfig.json` extends `$app/tsconfig`, which the SvelteKit-generated `.svelte-kit/` supplies — so a copy reports a different `fan_in` and dead-export count than the real tree, even with `.svelte-kit/` copied and `node_modules` hardlinked. Trust a cross-directory run for the *shift* between two penalty rows, never for the absolute score: prove the final number by re-running in `web/` itself.

## Fix your own dead code

- Generated shadcn folder: `bunx shadcn-svelte@latest add popover` writes nine files, but a caller usually imports `Root`, `Trigger` and `Content` only. Delete `popover-close`, `popover-description`, `popover-header`, `popover-title`, keep `popover-portal.svelte` internal to the content file, and reduce `index.ts` to the imports plus `export { Root, Content, Trigger };`. Do the same for any other component you vendor: what nothing imports is a dead-file penalty.
- A type alias over the generated schema is the usual dead-export hit: `export type TokenSink = components['schemas']['TokenSinkDto']` with no consumer is worth `-0.1` alone. Delete the alias in `src/lib/api/types.ts`, never the schema entry (`schema.d.ts` is generated by `bun run generate:api`).
- A helper only called by a sibling in the same module loses its `export` instead of being deleted (`async function enablePush()` beside `sendTestPush`).
- A file that is served to the browser and never imported (`web/static/sw.js`) takes `// fallow-ignore-file unused-file` as its first line; deleting it would remove runtime behaviour the analyzer cannot see.
- Never lower `--min-score` or add an ignore pattern for the directory you just added; the gate exists to catch exactly that.

## The web stage budget

`scripts/check` skips the web stage while `web/`'s inputs are unchanged, so the stage's time budget is only tested when you touch `web/src`. When it overruns (`budget exceeded: web 224581ms > 180000ms`), time the sub-commands separately before blaming the host: `bun run build` is ~60 s, `svelte-check --incremental` is seconds warm, and Vitest dominated. Vitest's default forked pool re-pays the Svelte compile per worker file; `pool: "threads"` in `web/vite.config.ts` took the suite from 138 s to 106 s and the stage from 224 s to 163 s in a 193 s check. `--no-isolate` shortens it further but fails the tests that share module state. The budget is never raised.
