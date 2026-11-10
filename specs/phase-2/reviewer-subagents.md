# Reviewer subagents

Status: implemented. Runtime ownership and acceptance checks are recorded below.

## Confirmed scope

- Provide shared subagent spawn, steer, stop, and result retrieval tools, with Spec reviewer and Standards reviewer presets. Reviewer requests require the path to the ticket being reviewed.
- Do not introduce Runs, a ticket scheduler, or additional agent roles. The existing scout/worker proposal is outside this change.
- The implementing agent explicitly starts both reviewers after implementing each ticket, following skill instructions. The daemon does not infer implementation completion.
- Reviewers work independently and return separate reports. The same two reviewer Sessions and conversations persist across all tickets in the specification assigned to the implementing agent. If the assignment is only one ticket, the pair covers that ticket. Returning a report or moving to another ticket does not replace either reviewer. Use existing billion-context folding rather than resetting context.
- Identify each reviewer by parent Session and reviewer role, without a separate spec or assignment identifier. Repeated spawn requests reuse that reviewer; a different ticket path does not create a fresh reviewer. Continue follow-up work through the existing Session. A new main Session gets its own pair.
- Planned daemon updates resume active reviewer Sessions through the same continuation mechanism as other agents; idle reviewers stay idle. Stop, failure, and unplanned restart preserve reviewer Sessions for explicit continuation without automatic retries.
- Include uncommitted changes in reviews. Capture the implementation starting point before making changes; the parent pauses edits while reviewers inspect the completed changes.
- Give reviewers a `submit` tool. Every submission delivers a report to the parent through a new steer; delivery is not limited to the first submission. Reviewers can submit multiple times without losing their Session. Keep explicit result retrieval available.
- Only `submit` forwards review reports. Ordinary final answers stay in the reviewer Session; error notifications do not depend on submission.
- Failed or stopped reviews report their status rather than a passing result. On a reviewer error, prompt the main agent to tell the user.
- The implementing agent presents the reports. There is no automatic fix/review loop or ticket closure.
- Global Settings store a model and reasoning effort independently for each reviewer. These defaults apply when creating reviewer Sessions, not retroactively. Reuse existing model controls; do not add per-ticket configuration.
- Show both reviewer Sessions under the main Session in the sidebar. The user can open each reviewer, change its model explicitly, and prompt it to continue in the same Session after an error.
- Derive the reviewer prompts from `.agents/skills/code-review/SKILL.md`. Preserve the separation between Spec and Standards findings. Adapt the skill to create fresh reviewer Sessions only initially, then reuse them across tickets and follow-up reviews.

## Tool policy

Reviewers do not receive the `edit` tool. Their instructions prohibit editing code. Other tools remain available so reviewers can run checks and investigate as needed. Do not turn this into an enforced read-only sandbox.

This is a behavioral restriction, not filesystem protection: shell commands and other available tools can modify files, and checks may generate artifacts.

## Submission contract

Reuse the Go child-submit shape, not its Run reviewer verdict schema:

- `status`: required, `done` or `blocked`, with surrounding whitespace trimmed.
- `report`: Markdown, at most 12000 Unicode characters.
- `summary`: at most 300 Unicode characters, normalized to one line.
- `done` means the review work is complete, not that the implementation passes review.
- Validate at the tool boundary. Oversized content returns an error asking the reviewer to shorten it; a rejected call does not deliver a report.
- Each accepted invocation creates its own submission identity and enqueues a new parent steer. Independent invocations with identical content are still distinct submissions.
- Route every submission through the existing steer queue, whether the parent is active or idle. Do not gate delivery on the child finishing or on both reviewers finishing. Queuing a steer is distinct from the parent consuming it.
- Preserve each report and its sender, ticket path, and submission identity for retrieval and UI rendering. A later report does not overwrite an earlier report.

## Report presentation

Subagent reports look like incoming subagent messages, not messages authored by the user:

- An inline report row headed `Sub-agent report`, with the reviewer role and a link to its Session.
- The one-line summary is visible; an accessible disclosure reveals the Markdown report.
- Show the ticket path and `done` or `blocked` status. Keep delivery state visible using the existing queued/delivered behavior.
- Keep Spec and Standards reports separate. Render each submission as a separate report, including later submissions from the same reviewer.
- Show reviewer errors as distinct error notices with a link to the reviewer Session. Do not label review completion as a passing verdict.
- Reuse the current feed typography, color tokens, and house components. Preserve mobile layout, keyboard focus, contrast, and reduced-motion behavior; do not add a separate messaging dashboard.

## Go reference findings

The reference checkout is `/root/projects/emberloop`, inspected read-only with the user's permission.

- `emberloop-web/el/tools/submit.go` defines child-submit validation and text limits.
- `emberloop-web/el/tools/webproxy.go::webProxyChildSubmit` sends the child payload to the Session submit route.
- `emberloop-web/runs_exec.go::handleSessionSubmit` accepts child reports, deduplicates identical content, and asks the Group delivery machinery to deliver them.
- `emberloop-web/report_delivery.go::findReportDeliveryForLocked` identifies an existing delivery by Group and child, rather than by submission. `enqueueGroupReports` skips a child with an existing delivery, and `enqueueReport` returns without creating another delivery. This is a one-shot delivery path that must not be copied for long-lived reviewers.
- The Go bug was inspected in source, not reproduced against a running daemon. No Go code was changed.

## Acceptance checks for implementation

- Repeated spawn requests, including requests for another ticket, return the same reviewer Sessions for one parent. A new parent gets a separate pair.
- Three sequential submissions from one reviewer produce three distinct parent steers and three report rows; repeat with identical report text.
- Submissions deliver while the parent is active, while it is idle, and across a transition from active to idle. Reports from both reviewers remain distinct.
- A queued submission survives restart. A planned update resumes both active reviewer roles with their existing identity and conversation, without waking idle reviewers. Stop, failure, and unplanned restart require explicit continuation.
- Invalid status and oversized report or summary are rejected without delivery. Verify Unicode character limits and summary normalization.
- Ordinary final answers do not forward reports. Reviewer errors notify the parent with instructions to tell the user.
- Reviewer Settings defaults apply only at creation. A user can change an existing reviewer's model and prompt that same Session after failure.
- The sidebar links both reviewers under their parent. Report disclosure, repeated reports, error notices, and queued/delivered state render accessibly.
- Reviewers lack `edit` but retain check-running tools. Review prompts retain both original review axes and instruct reviewers not to edit code.
- Reviews include uncommitted work and use the captured implementation starting point. The skill reuses reviewers across tickets and keeps the parent from editing during review.

## Runtime ownership

`crates/api/src/subagents.rs` owns reviewer actions and report delivery. `SessionMeta.reviewer` persists assignments, and the shared steer queue persists separate report entries. The proxy pins the two role prompts from the Prompt Store; tool and wire filtering remove only `edit`. The daemon passes Session policy to MCP and restores report delivery without resuming reviewer work.

`session_setup.rs` validates new reviewer defaults without configuring a nonexistent harness. The pi harness forwards explicit Session variables in its stdio MCP configuration while keeping the pi process environment scrubbed.

The web sidebar groups reviewer children under their parent. `SubagentReport.svelte` renders report metadata retained by the feed reducer. Global Settings expose creation-only model and reasoning defaults; like the existing Settings, these defaults are process-local. Existing reviewer model choices persist with their Session metadata.

The `implement`, `implement-spec` and `code-review` skills dispatch reviews after each ticket without introducing Runs. The role prompts now live in the Prompt Store instead of being duplicated in the dispatch skill.

## Sources inspected

- `.agents/skills/code-review/SKILL.md` defines the two review axes and their prompts.
- `.agents/skills/implement/SKILL.md` invokes code review after implementation.
- `crates/proxy/src/review.rs` and `crates/daemon/src/review.rs` implement background skill maintenance, not these code reviewers.
- `specs/phase-2/spec.md` lists scout/worker subagents as proposed scope.
