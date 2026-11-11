# AGENTS.md — emberloop-rs

Project preamble: emberloop-rs is a Rust rewrite of Emberloop, started fresh and not a port. Tools are served over MCP, and one proxy owns everything on the model wire (billion context, Ponytail text, steering). Every harness plugs into both through one small trait. Phase 1 is `agy` only, with Projects, Sessions, `read`/`edit`/`bash`/`background`/`grep` and a trimmed copy of the Emberloop UI (SvelteKit, Tailwind v4, shadcn-svelte). The Go Emberloop at `~/Projects/emberloop` is the reference for behaviour and is not changed from here. The spec is the source of truth; implementation lives in `crates/` and `web/`.

Web tooling is bun: `bun`, `bunx` and `bun.lock` everywhere, never npm or another package manager. Every control in `web/` is a shadcn-svelte component over `bits-ui`: add one with `bunx shadcn-svelte add <name>` and use it as generated, never a native `<select>`-style widget or a hand-rolled one.

## DOX contract

- `AGENTS.md` files are binding work contracts for their subtrees. A path must stay understandable from its nearest `AGENTS.md` plus every parent above it.
- Read before editing: read this root, walk from the root to each target path, and read every `AGENTS.md` on each route, following the child index down. The nearest doc is the local contract; parents carry repo-wide rules. Never rely on memory of these docs.
- Update after editing: a change that moves purpose, ownership, contracts, structure, workflow, inputs, outputs, or permissions must update its nearest owning doc, plus affected parents and children, in the same task. Refresh every affected child index and delete stale text on the spot. Trivial edits with no contract change need no doc change, but the pass itself still happens.
- Hierarchy: this root states project-wide rules and indexes the top level; children own domain detail and their own index. Closer docs are more specific. No child may weaken this contract.
- Style: concise, current, operational. Broad rules live in parents, concrete names in children. Never duplicate a rule across docs; each rule has one home. No history, no obvious statements, no warnings for dead risks.
- Closeout: re-check changed paths against the chain, update the owning docs, refresh indexes, run the check when code changed.

## Check

One command, `scripts/check`, is the repo check and must pass before any change is done. `.github/workflows/ci.yml` runs the same stages on a fresh checkout, except the Fallow complexity check and the time budgets (both are development-host checks). It runs, in order:

- `cargo fmt --check`, `cargo deny check` and `cargo machete`, then `cargo clippy --workspace --all-targets -- -D warnings` with the `[workspace.lints]` table (including `cognitive_complexity`, its threshold in `clippy.toml`);
- the OpenAPI document is dumped, the TypeScript client regenerated, and `git diff --exit-code` passes; an unchanged client keeps its modification time;
- in `web/`: `bun install --frozen-lockfile`, `svelte-check --incremental` (which covers `tsc`), a blocking Fallow complexity check with the Go repo's thresholds, the web tests and `bun run build`, because the daemon embeds `web/build/` at compile time and `crates/daemon/tests/web_bundle.rs` fails on a bundle older than `web/src`;
- `cargo nextest run --workspace` and `cargo test --doc`, after the bundle is built so the staleness test sees a fresh one;
- the slowest tests.

The check has a time budget (spec decision O): the loop check 30 s or less, the full check 3 minutes or less. A slow test, a real sleep or a real harness in the check is a defect. No stage is skipped, softened with `|| true`, or guarded by "if the tool exists".

The web stage is skipped when `web/`'s input tree hashes the same as the green run before it, `web/build/` exists and holds no file older than a web source: same inputs, same result. A missing or stale build always runs it. Every other stage always runs.

A threshold is never raised to make a change pass; the function is split instead.

## Release and update

Everyone updates with `emberloop-daemon update`, in a Session or from a terminal: it only queues an orderly restart. Run `scripts/check`, then `scripts/release` to build the next version into `target/release-dist`; `scripts/release --publish` uploads it as the GitHub release `emberloop-daemon update` reads. For a build not yet released, `emberloop-daemon update --file <binary>`. Do not stop the daemon to force the update. See `crates/daemon/AGENTS.md` for the update, resume, and verification contract.

## Test-first

All code is written test-first: a failing test that states the behaviour comes first, then the smallest code that passes it, then a refactor with the test green. A ticket's acceptance checkboxes become tests before any implementation, and a change without a test that failed first is not done. A bug fix starts with a test that reproduces it. Spikes (throwaway code that answers a question) are exempt, but their findings become fixtures and tests before the real code is written.

## Research first

When asked to develop something (a feature, ticket, subsystem or integration), do not start by writing code. First research online for existing tools, crates, libraries and prior art that could do the job or speed it up, using primary sources (repos, docs, crates.io or npm pages). Then report to the User, and wait for the User's decision before implementing:

- A table of candidates, each rated **0 to 10 for fit** to this project (the project rules and the spec decisions are the yardstick), with one line of reasoning, the last release date, maintenance status, and the cost of adopting it (dependency weight, build time, RAM, licence).
- A recommendation, including "build it ourselves" as a candidate when that rates higher.
- What could not be verified from a primary source, stated plainly.

Only after the User picks does implementation start, test-first. This does not apply to a bug fix, a refactor or a change that adds no dependency or tool, nor to a choice a spec decision has already fixed; it applies again when a spec decision is reopened.

## Project rules

- A harness is one implementation of the harness trait. It never composes model-facing text; the proxy and the one policy text own it. A new capability is declared once and read by every harness, never added as a per-harness branch.
- Plugins are compiled-in workspace crates on a Cordis-style Context tree. No dylib, WASM or runtime plugin loading until a second plugin author exists.
- Tools reach a harness over MCP only. The proxy never executes a workspace tool for a harness; it runs only its own pure tools (compress, decompress, search_context). One exception: the background skill review fork runs its tool calls through an executor the daemon injects (`ReviewTools`); the proxy crate has no tools dependency.
- A third-party stdio MCP server listed in `<config root>/mcp.json` is reachable only from the `execute` tool, never declared to a harness, and the proxy only adds its skill text when the daemon reports the code graph usable.
- Pre-0.1 releases are 0.0.x. Until v0.1, an update may drop Sessions and data-root layout without migration; that is not a defect and needs no compatibility code.
- No auth, loopback bind only. A non-loopback bind needs an explicit flag.
- Claude runs as the real `claude` binary through the proxy's `claude` route; the proxy forwards the client's own Authorization header in transit and never stores, logs or generates a token.

## Child DOX Index

Index paths are repo-relative. Every doc below is listed here or in the child it belongs to, and every entry must resolve to a file. An entry routes: it names what a subtree owns and when to open it, and keeps no rule that the child states.

- `specs/AGENTS.md` — the specs and the UI mock: phase 1 (agy, Sessions, core tools, one proxy), its tickets and acceptance run
- `crates/AGENTS.md` — Cargo workspace crates implementation
- `web/AGENTS.md` — SvelteKit SPA: Projects sidebar, Session feed, image composer, model/context dialog, Settings
