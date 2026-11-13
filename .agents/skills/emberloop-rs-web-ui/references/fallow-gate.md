# The Fallow health gate

`cd web && bunx fallow health --score --min-score 87.9` must pass before a web change is done, and it is a `scripts/check` stage. The number is `100 − sum(penalties)`, so a failing run is always one new deduction away from the baseline.

## Read the score, not the headline number

- Judge with `--score`, which is the gate. Plain `bunx fallow health` also charges `hotspots -10.0` and reports a much lower grade (`77 B`) with no change to the code.
- Which categories cost what: `bunx fallow health --format json` returns `health_score.penalties` (`dead_files`, `dead_exports`, `unit_size`, `coupling`, `complexity`, `hotspots`, …).
- What exactly is dead: `bunx fallow check --format json` returns `unused_files` and `unused_exports` with `path`, `line` and an `actions` list (delete, or suppress with a comment).
- `bunx fallow check` also prints advisory config notes (`entry pattern matched only files that are entry points anyway`, `ignorePatterns entry excluded no file`); those are pre-existing noise, not findings.

## Find out which penalties were already there

A healthy tree scores about `88 A` against a `87.9` floor, with `unit size -10.0 · coupling -1.4 · complexity -0.4` already deducted: `unit size` charges the 600-line limit on `src/routes/+layout.svelte` and the generated `src/lib/api/schema.d.ts`, and neither is yours to split for a ticket. Measure instead of guessing:

```bash
git worktree add /tmp/base <commit-before-your-change>^
ln -s "$PWD/web/node_modules" /tmp/base/web/node_modules
cd /tmp/base/web && bunx fallow health --score --min-score 87.9
```

Compare the penalty lines with the current run. Deductions the baseline already had are debt; the delta is what you fix. Remove the worktree afterwards.

## Fix your own dead code

- Generated shadcn folder: `bunx shadcn-svelte@latest add popover` writes nine files, but a caller usually imports `Root`, `Trigger` and `Content` only. Delete `popover-close`, `popover-description`, `popover-header`, `popover-title`, keep `popover-portal.svelte` internal to the content file, and reduce `index.ts` to the imports plus `export { Root, Content, Trigger };`. Do the same for any other component you vendor: what nothing imports is a dead-file penalty.
- A helper only called by a sibling in the same module loses its `export` instead of being deleted (`async function enablePush()` beside `sendTestPush`).
- A file that is served to the browser and never imported (`web/static/sw.js`) takes `// fallow-ignore-file unused-file` as its first line; deleting it would remove runtime behaviour the analyzer cannot see.
- Never lower `--min-score` or add an ignore pattern for the directory you just added; the gate exists to catch exactly that.

## The web stage budget

`scripts/check` skips the web stage while `web/`'s inputs are unchanged, so the stage's time budget is only tested when you touch `web/src`. When it overruns (`budget exceeded: web 224581ms > 180000ms`), time the sub-commands separately before blaming the host: `bun run build` is ~60 s, `svelte-check --incremental` is seconds warm, and Vitest dominated. Vitest's default forked pool re-pays the Svelte compile per worker file; `pool: "threads"` in `web/vite.config.ts` took the suite from 138 s to 106 s and the stage from 224 s to 163 s in a 193 s check. `--no-isolate` shortens it further but fails the tests that share module state. The budget is never raised.
