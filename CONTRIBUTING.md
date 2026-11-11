# Contributing to emberloop-rs

emberloop-rs runs coding-agent Sessions from a local daemon and proxies their
model traffic. This file covers what a change has to satisfy before it is
merged; the code-level contract lives in the `AGENTS.md` files.

## Before you start

- Read the root `AGENTS.md`, then the nearest `AGENTS.md` of every directory you
  touch. Those files are binding: a change that moves structure, ownership,
  contracts or workflow updates them in the same pull request.
- Open an issue first for anything larger than a bug fix or a small polish. The
  maintainer works on `master` and merges work that fits the current design.
- The specification is the source of truth for behaviour. If your change
  contradicts `specs/`, say so in the issue before writing code.

## Requirements

- Rust (current stable) with `rustfmt`, `clippy`, `cargo-nextest`,
  `cargo-deny` and `cargo-machete`
- `bun` (this repository is bun-only: never npm, pnpm or yarn)
- `git`

## Development loop

```bash
scripts/check-changed  # the stages the working tree touches
scripts/check          # everything: fmt, deny, machete, clippy, openapi, web, nextest, doctests
```

`scripts/check` is the repository check and CI runs the same commands. It must
pass unchanged; never weaken a stage to get a change through. Per-stage logs are
`target/<stage>.log`, and `target/<stage>.failed` marks a failed stage.

Two things that catch people out:

- The daemon embeds `web/build` at compile time, so a fresh checkout needs
  `cd web && bun install --frozen-lockfile && bun run build` before any cargo
  stage that links the daemon.
- The `openapi` stage regenerates `openapi.json` and
  `web/src/lib/api/schema.d.ts`; the `generated` stage fails until those two
  files are staged.

To run the daemon locally:

```bash
cargo run -p emberloop-daemon -- --data-root /tmp/emberloop-dev
```

It serves <http://127.0.0.1:3000>.

## Commit messages

`type(scope): summary` with `feat`, `fix`, `refactor`, `perf`, `build`, `docs`,
`test`, `chore`. Imperative summary under 72 characters; the reason goes in the
body. Do not mix unrelated changes in one commit.

## Pull requests

A pull request should:

- state what changed and why, in the body
- say which command you ran and quote its result line
- link the issue it closes, if any
- update `AGENTS.md` and user-facing docs when the change moves a contract
- include the rebuilt `web/build` output when UI source changed
- avoid reformatting files it does not otherwise change

Screenshots or a short recording help for UI changes.

## Tests

- Add a test for every behaviour change; a bug fix needs a test that fails
  without it.
- Behaviour is tested through the HTTP and proxy surfaces where possible, with
  the unit test next to the code it covers.
- Live tests that reach a real model or a real harness CLI are `#[ignore]`d or
  gated by an environment variable; run one deliberately when you touch that
  path.

## Licensing of contributions

emberloop-rs is dual-licensed under Apache-2.0 and MIT ([README](README.md#license)).
By opening a pull request you license your contribution under the same terms. No
CLA and no sign-off trailer are required.
