---
name: emberloop-rs-ship-workflow
description: Use when finishing an emberloop-rs change and you must verify, build a release, and update the running daemon.
metadata:
  origin: agent
---

# Ship an emberloop-rs change

Steps, in order. Read a step's reference before the step, not after.

1. Read the AGENTS.md chain for every crate you touch before editing.
2. Iterate with focused tests, then `scripts/check` with `background: true` and one wait. It must pass unchanged; never weaken it. Its `generated` stage needs the regenerated `openapi.json` and `schema.d.ts` staged, and staging is enough: ask before committing. Detail: `references/checks-and-tests.md`.
3. Build with `scripts/release`, setting `EMBERLOOP_HOME` and `HOME` explicitly. It stops before publishing: do not add `--publish` unless asked, and say in the reply that it was not published. Detail: `references/release-and-install.md`.
4. Queue the install with `target/release/emberloop-daemon --data-root <data root> update --file "$PWD/target/release/emberloop-daemon" --session <current session id>`. It only queues: the daemon restarts the Session once idle, so verify after that restart, not right after the command.
5. Prove the new release is live (`current` target, `--version`, `/api/version`, and for `web/` a fingerprinted artifact plus a live page check): `references/release-and-install.md`.
6. For a behaviour change that reaches a live model, test it on a real Session: `references/live-session-testing.md`.

Delegation to a `worker` (tier, brief, one wait): `references/harness-and-sessions.md`.

## Where the detail lives

- `references/checks-and-tests.md`: focused test commands, `scripts/check` stages and budget, fmt and clippy, nextest timeouts and races, fixture contracts, proxy fault fixtures.
- `references/release-and-install.md`: version pinning on `--publish`, artifact checks, the queued install, and the live bundle, SSE and page proofs.
- `references/harness-and-sessions.md`: the worker brief, the update restart that deactivates reviewers, handle caching, ACP probing, tracing, token accounting, provider auth state.
- `references/repo-edits-and-commits.md`: hashline patch rules, checkpoint commits of a mixed checkout, a checkout shared with another agent, pushes, documenting committed behaviour, Mermaid renders, tmpfs builds, shell allowlists.
- `references/model-facing-text.md`: the five places tool behaviour reaches, subagent limits, proving prompt text from the binary, `el://docs` braces, reference-doc verification.
- `references/web-and-ci.md`: Svelte prop and schema fixture breaks, proxy event payloads, CI step order, the fallow gate pointer.
- The older references stay where they were: `baked-in-subsystems.md`, `dependency-upgrades.md`, `live-session-testing.md`, `new-harness-tool.md`, `pi-ai-pin.md`, `provider-auth-and-quota.md`, `proxy-failure-fixtures.md`, `shared-tree-commits.md`.
