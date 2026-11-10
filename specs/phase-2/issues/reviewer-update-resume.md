# Resume active reviewers after a planned daemon update

## Request and evidence

Reviewer Session `ses_10b72ff4517c` stopped during the `0.0.70` to `0.0.71` update. Its harness recorded `MCP request aborted` at 2026-11-10 00:34:36 UTC. The update restarted the daemon one second later. The Session received no continuation until the user returned over six hours later. Startup explicitly excludes reviewers from the existing post-update resume list.

The user confirms: reviewers use the same mechanism as other agents. Sessions that are not idle when a planned update interrupts them resume afterward.

## Scope

Remove the reviewer-only startup exclusion. Keep the existing update selection and continuation path: the requesting Session receives the update message; other interrupted active Sessions receive `continue`. Idle reviewers are not added to the list. No new scheduler, retry loop, tool timeout, UI repair, dependency, or persistent format.

## Acceptance

- [x] Post-update startup resumes active ordinary Sessions and both reviewer roles through the same path, preserving their Session identities and conversation tokens.
- [x] Idle reviewers are not resumed. Stop, failure, and unplanned restart still require explicit continuation.
- [x] Regression fails before the fix and passes afterward at the daemon startup/harness boundary.
- [x] Operational docs and reviewer specification distinguish planned update continuation from automatic retries.
- [x] `scripts/check` passes unchanged; independent Spec and Standards reviewers report separately before release.
- [x] Release is installed and its version verified after restart; exercise active reviewer continuation live.

## Baseline and ownership

- Implementation starting point: `5b429e512922631ad099e162cd10345172631eba`.
- Shared dirty checkout. Before this ticket, `crates/daemon/src/lib.rs` has 35 added and 1 removed lines for reviewer wiring, report recovery, and the reviewer exclusion. `crates/daemon/AGENTS.md` has 5 added lines; `specs/phase-2/issues/AGENTS.md` has 1 added line for the earlier fold ticket. Preserve all unrelated changes, including the completed folded-proxy fix and other reviewer/UI/tool work.
- `crates/daemon/tests/acceptance.rs` and root `AGENTS.md` are initially unchanged from HEAD. `specs/phase-2/reviewer-subagents.md` is pre-existing and untracked; this ticket changes only its restart rule and matching acceptance line. `crates/daemon/src/runner.rs` already has reviewer wiring changes; this ticket only corrects its two-line update comment.
- Test seam: existing daemon acceptance tests with a fake ACP harness and persisted planned-update marker, observing ordinary and reviewer continuation after serving starts.
- Keep this ticket open for the user. Do not commit without authorization.

## Verification

- Red: `cargo test -p emberloop-daemon --test acceptance planned_update_resumes_active_agents_and_reviewers_but_not_idle_reviewers -- --exact --nocapture` fails with `post-update continuation missing`: the ordinary Session receives `continue` on `saved-ordinary`, but both reviewer captures are empty.
- Green after deleting the eight-line exclusion: the same regression passes in 0.54s, checks exactly one continuation per active Session with its existing token, verifies the requester's update prompt, and leaves an idle reviewer untouched.
- Focused daemon checks: 51 library tests, 9 acceptance tests, and 16 runner tests passed. `cargo fmt --all -- --check` and `git diff --check` pass.
- Required check: `scripts/check` exited 0 and passed cleanly in 36s. The unchanged web inputs used the normal green-cache skip. Generated OpenAPI/client files are unchanged.

## Independent reviews

- Spec reviewer `ses_9fba548ed3f1`: pass. No missing requirements, scope creep, or incorrect implementation. Reviewer independently reran `scripts/check` successfully in 21s.
- Standards reviewer `ses_8a24a030274d`: clean. No documented-standard violations or baseline code smells.
- Both reports arrived before the release build. The ticket remains open for the user.

## Release and live probe preparation

- `EMBERLOOP_HOME=/root/.local/share/emberloop-rs HOME=/root scripts/release` exited 0. Candidate version `0.0.72`, SHA256 `37806a3a7bee17492f81f24504fa771ee8d90cd34b373643a80da170042643d3`. This local build includes pre-existing checkout changes; no commit or publication was made.
- Dedicated probe parent: `ses_69fba00e3e33`. Its real Spec reviewer `ses_eaa36cb1f2f1` and Standards reviewer `ses_324c2abfb4fc` both reached `background wait(timeout=600)` at event seq 15, waiting on one bounded `sleep 180` each. Both use `gemini-3.8-flash` and already have saved resume tokens. No source changes are requested from the probes.
- Idle controls immediately before update: Spec `ses_9fba548ed3f1` last event seq 197, Standards `ses_8a24a030274d` last event seq 164; both status `idle`.
- Pre-update resume-token SHA256: active Spec `1a2bc75d851cb55076a54a6645748e8f9874856c9c3aa1e19f8690d8d7bc4dd6`; active Standards `96f8e6c053096ea95efb3fa510eeabbd318f9d29fc968d71fa12d277f0bbbd0c`. Compare after restart without exposing raw resume tokens.
- Expected automatic completion markers: `LIVE_UPDATE_RESUME_SPEC_OK` and `LIVE_UPDATE_RESUME_STANDARDS_OK`, delivered as separate reports to the dedicated parent. Installation, token continuity, active continuation, and idle exclusion remain to verify after restart.

## Installed release and live results

- Installed `0.0.72`: `current -> releases/0.0.72`, installed binary `--version`, `/api/version`, and serving PID `3597828` executable `/root/.local/share/emberloop-rs/releases/0.0.72/emberloop-daemon` all agree. Installed binary SHA256 matches the release candidate above.
- Both active reviewers resumed automatically without a manual prompt. Their Session IDs and resume-token hashes match the pre-update baseline. Both returned idle and delivered separate `done` reports to the dedicated parent: Spec `submit_e2d820aabe1b` with `LIVE_UPDATE_RESUME_SPEC_OK`; Standards `submit_6c887d24e38b` with `LIVE_UPDATE_RESUME_STANDARDS_OK`. Report delivery states are both `delivered`.
- Idle controls remained idle with no new events: Spec seq `197`, Standards seq `164`. Planned updates now resume active reviewers through the existing path without waking idle reviewers.
- No original incident records or stale tool rows were repaired. No commit or publication was made; this ticket remains open for the user. Final `git diff --check` passed.
