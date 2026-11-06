# Ticket 21: Universal version checks, Settings -> Versions

## Goal

One small registry of "things whose version matters" and one scheduler that checks them all every 4 hours, so adding a new check is one entry and no new timer. Settings shows each entry's pinned version, the latest upstream version, and a status. Nothing updates itself. A User who sees "newer upstream" prompts a reviewer to decide whether the change matters, and updates by hand. Blocked by ticket 10; the first entries come from tickets 20 and P2-1/P2-2.

## Status

Not started.

## Decisions

- **No auto-update, ever.** Every external tool is pinned to an exact version. A check never downloads, installs or restarts anything.
- **Two kinds of entry, one shape.** A `tool` entry is something we run (pi, claude, agy, `emberloop` itself). A `reference` entry is something we ported behaviour from and must keep in step (billion-context). Both carry `pinned` and `latest`; a reference entry also carries the baseline recorded in its parity files, and its status wording says "review", not "update".
- **One scheduler.** A single background task owns the interval (default 4 h, configurable, minimum 15 min), runs every enabled check in turn with a per-check timeout, and never blocks startup. Jitter of up to 10 percent so several daemons do not hit a registry together.
- **Sources are pluggable, tiny and read-only:** `npm` (registry `dist-tags.latest` and `time.modified`), `github` (latest release tag), `crates` (max stable version), `command` (run `<tool> --version` locally, for the installed side). A new source is one function. Network sources use plain HTTPS GETs with a short timeout; offline means the last result is shown stale, with its age.
- **Result is data, not UI logic.** `GET /api/versions` returns entries with `{id, name, kind, source, pinned, installed, latest, checkedAt, status, note}`; `POST /api/versions/check` forces a run. Status is one of `current`, `behind`, `review` (reference moved), `unknown` (never checked or source failed). The SPA only renders it.
- **Persistence:** `<data root>/versions.json`, atomic rename, last result per entry; a restart shows the last known state at once and rechecks at the next tick.
- **The manual path stays human.** A `review` or `behind` row shows the upstream version and a link to its release page, plus a "Copy review prompt" button that puts a ready-made instruction on the clipboard (what changed since the baseline, what in this repo depends on it, what to re-run: for billion-context, `crates/proxy/parity/regen.sh` against the new package and a diff of the golden files). The User pastes it to a reviewer; nothing is sent anywhere by the app.
- **Acknowledge, not auto-bump.** After review, the User raises the pinned baseline by editing the one file that holds it (`crates/proxy/parity/BASELINE` for billion-context, the pin file for each tool). The Settings row has no "update" button in this ticket.

## Registry (first entries)

| id | kind | source | pinned from |
|---|---|---|---|
| `billion-context` | reference | npm `billion-context` | `crates/proxy/parity/BASELINE` (0.1.212 on 2026-11-06) |
| `emberloop` | tool | github release of this repo | the running build |
| `pi` | tool | github `yannelli/pi-rust` | the pin in `crates/harness-pi` |
| `claude` | tool | `command` (`claude --version`) plus npm `@anthropic-ai/claude-code` | the version the stream-json fixtures were recorded with (P2-2) |
| `agy` | tool | `command` | the version in the agy harness notes |

A harness crate registers its own entry when it is wired in; the registry is a `Vec<VersionCheck>` built in the daemon composition root, so a plugin adds one line.

## Tests first

Failing before implementation, with a fake source and a fake clock (no network, no real sleep):

- Every enabled check runs once per tick; a failing source marks only its own entry `unknown` and keeps the previous `latest`.
- A tick that overlaps a still-running check skips that entry instead of stacking.
- The interval and minimum are enforced; jitter stays inside 10 percent.
- `current` when pinned equals latest; `behind` for a tool with a newer upstream; `review` for a reference whose upstream moved past its baseline; `unknown` before the first run.
- Version comparison handles semver with pre-release tags and ignores `pr-*` dist-tags (billion-context publishes many).
- `versions.json` round-trips and a corrupt file is replaced, not fatal.
- `POST /api/versions/check` runs all checks once and returns the new list; the OpenAPI document and generated client include both routes.
- Web: the Versions panel renders each status, shows the age of the last check, disables nothing while checking, and the copy button writes the review prompt text (fake clipboard).

## Out of scope

Updating anything, release notes fetching, security advisories, per-Project pins.

## Docs to update in the same change

`crates/daemon/AGENTS.md` (scheduler and registry), `crates/api/AGENTS.md` (routes), `web/AGENTS.md` (Versions panel), `specs/phase-1/issues/AGENTS.md`.
