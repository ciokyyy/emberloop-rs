---
name: ember-audit
description: Use when a design document (usually .scratch/<slug>/design.md from ember-design) must be checked for holes before tickets or code. Runs Elephant-Goldfish review stages with fresh-context goldfish personas (zoom-out, comprehension, critic, implementation readiness), then resolves each finding with the user.
metadata:
  origin: agent
---

# Ember audit

The elephant (this Session) wrote the design and agrees with it, so it cannot see its own holes. Goldfish reviewers can: each is a fresh, read-only, one-shot subagent that reads only the files you name. A review done in this Session is not a goldfish review.

The personas are the unmodified EGM files in this skill's `agents/` folder. Their standards are in `resources/document_standards/`.

## Setup

1. Target: the absolute path of the design. Read it and find the line `L` of `## Detailed implementation plan` (also accept `## Implementation plan`, `## Work breakdown`, `## Implementation steps`). `FeatureRange` = lines `1..L-1`; `DetailedRange` = `L..end`. No such heading: `FeatureRange` = whole file, skip stage 4, tell the user.
2. Context files: absolute paths the reviewers may read to learn the system. Add `decisions.md`, `GLOSSARY.md`, the ADRs the design cites, and every file the design links that exists inside the Project. Ask the user about links outside the Project; a goldfish can read only the Project and the skill roots. Save each needed URL as a file under `.scratch/<slug>/audit/sources/` and pass that path.
3. Standards: `$HOME/.agents/skills/ember-audit/resources/document_standards/<name>.md`, expanded to an absolute path.
4. Mode: by default stop after each stage. When the user says "run all", or asks to continue until their input is needed, run every stage without stops (spawn stages 2 to 4 together in one `execute` loop, since they do not depend on each other's reports) and write one consolidated report; stop at the Resolve step, which needs the user's answers.

## Stages

Run stages in order. Inside a stage, spawn its goldfish together in one turn.

| Stage | Personas | Scope | Standard | Context files | Report |
| --- | --- | --- | --- | --- | --- |
| 1 Zoom out | `egm-zoom-out`, `egm-inventive-catalyst` | whole file | `zoom-out-standard` | yes | `audit/1-zoom-out.md` |
| 2 Comprehension | `egm-comprehension` | `FeatureRange` and `DetailedRange` | none | none: it reads the document alone | `audit/2-comprehension.md` |
| 3 Critic | `egm-design`, `egm-structural`, `egm-style` | `FeatureRange` | `feature-spec-standard` for design and structural | yes | `audit/3-critic.md` |
| 4 Readiness | `egm-implementation-readiness`, `egm-codebase-details`, `egm-style` | `DetailedRange` | `implementation-readiness-standard` for the first two | yes | `audit/4-readiness.md` |
| 5 Re-zoom | `egm-zoom-out` | whole file | `zoom-out-standard` | yes, plus `audit/1-zoom-out.md` | `audit/5-re-zoom.md` |

Run stage 5 only when fixes since stage 1 added more than a few lines; it checks that the fixes did not bloat the design.

Do not spawn `egm-synthesizer` or `egm-bootstrap-orchestrator`: a goldfish cannot spawn subagents. You are the synthesizer.

Spawn each goldfish as `spawn role=goldfish persona=<id> tier=mid` with this text:

```text
Mode: REVIEW
Target: <absolute path>, lines <a>-<b>
Standard: <absolute path>            (omit for egm-comprehension)
Context files: <absolute paths>      (omit for egm-comprehension)
Focus: <stage 5 only: review what changed since the stage 1 report>
For each finding give severity (Blocker, Major or Nit), the line range, the problem and why it matters.
Keep the report under 6000 characters, then call submit.
```

Reports arrive as steers; the goldfish stops itself after it submits. A goldfish that errors, stops or ends without a report did not review: retry it once, then record it as "not run". Retry with `retry_call` when the spawn was a native call; a spawn made inside `execute` has no failed call id, so rebuild the same text from `tools.exec_source(<exec n>)` and spawn again, one tier up when the failure was an upstream stream error. Keep the failed id in the poll loop too: an errored goldfish can still submit later, and its late report is a valid review to save and merge. Never count "not run" as zero findings. If the retry fails on a missing host binary, tell the user and wait; do not substitute a review done in this Session.

Save each report as it arrives to `audit/raw/<stage>-<persona>.md`: in `execute`, `json.loads(tools.call('result', id=<goldfish id>))['reports'][-1]['text']`, then `tools.write`. To collect several pending goldfish, poll them in one `execute` loop (check `result` for each id every 15 s, save each report as it lands, exit when none is pending) instead of one turn per arrival. The steer copy folds away with context, so the file is what the merge reads. Write the stage report yourself (a goldfish cannot write files): each finding with a stable id `<stage>-<persona>-<n>`, its severity, lines, problem and reason. Derive the severity counts from the finding headings rather than copying a persona's summary header, which can drift from its finding labels. The comprehension report starts with the persona's two paraphrase headings copied verbatim; a wrong paraphrase is a finding.

After each stage, link the report in chat with the count by severity and the one or two findings that matter most. Then ask: proceed, resolve findings now, or pause while the user edits the design.

## Resolve

Classify every finding as exactly one of:

- PROPOSE FIX: the finding is valid. Write the exact replacement text.
- CONTEST: the finding is wrong, and you can cite evidence the goldfish could have read (a file and symbol, or a line of the design). "This was intentional" is not evidence.
- ESCALATE: the finding is wrong, but the evidence exists only outside the files (a team convention, a spoken decision). Do not fix; give it to the user.
- DECLINE: a Nit not worth a fix. Record it.

Rules:

- You wrote the design. A wish to contest is not evidence that the goldfish is wrong.
- Never contest a comprehension gap. If a fresh reader cannot follow the design, the design lacks context.
- Add context because a human reader needs it, not only to satisfy a goldfish. When only a goldfish needs it, ESCALATE.
- Do not change a severity. Contest it with a reason instead.
- Before classifying a finding about remote or deployed state (migrations applied, secrets set, bindings, rows in production), query the live host read-only (for example `wrangler d1 migrations list DB --remote`, `wrangler secret list`). Goldfish read files only, so a Blocker built on a repo file can be false on the host: CONTEST it with the command output.
- A finding that adds a new decision or reverses a recorded decision in `decisions.md` goes back to the user as an `ember-grill` round (classify it `ESCALATE` with options rather than rewriting `design.md` silently), and updates `decisions.md` once answered.

Write `.scratch/<slug>/audit/resolution.md` with each finding, its class, the reason and the proposed edit. In chat, list numbered `ESCALATE` items first with concise options and a recommended pick, followed by a compact summary of `PROPOSE FIX`, `CONTEST`, and `DECLINE` items so the user can answer `"all recommended"`, `"accept all"`, per item, or `"skip"`. Apply approved edits, re-read the design, and recompute the ranges.

## Converge

- Apply all approved edits to the design in one `edit` patch against one tag, then grep the `##`/`###` headings and the changed claims to confirm they landed, and recompute `FeatureRange`/`DetailedRange` from the new `## Detailed implementation plan` line before any re-audit. When a fix inserts, removes or splits a target-file item, recompute every item-number cross-reference (phase lists, new-tests list, "see item N"), because numbering shifts silently and slices are cut from those lists.
- Before spawning Stage 5 (`re-zoom`), verify that new fixes did not invent a duplicate kill-switch flag, wrap a slow handshake in a shorter outer timeout, or scatter state-transition invariants across callers instead of the single mutator.
- Re-audit spawns reuse the stage text and add the line `This is a second pass: the author fixed the first findings (see audit/resolution.md). Report only what is still wrong.` with `resolution.md` in the context files.
- Merge the re-audit into one `audit/resolution-2.md`: a table of merged items (id, severity, finding, raw sources, class), then the fixes, then numbered `ESCALATE` items with options. List the live checks you ran before classifying at the top, so a CONTEST cites them.
- Re-audit a stage at most once after its fixes. A finding that is contested or accepted as a risk is settled: when a later goldfish raises it again at the same lines, do not re-open it.
- A fixed finding that comes back at the same lines is open again: the fix failed or a later edit undid it.
- After the re-audit, record the remaining findings under `## Open questions` or as accepted risks in the design, with the user's agreement.

- Goldfish verify against files as they read them, and "resolved" in a consolidated report means the text changed, not that it is true. Before you tell the user the plan is ready, re-check every concrete claim in the plan steps against the live tree and host in one batched check: version numbers (read the installed one, never trust a number written in the plan; grep the old version literals across workspace tests and `AGENTS.md` so test fixture paths match bumped fallbacks, while leaving historical wire snapshots intact), resolved binary paths (`which`/`readlink` vs fallback dirs — binaries often resolve from `PATH`), external HTTP URLs and JSON field paths (goldfish cannot fetch network URLs, so guessed endpoints survive audit; for vendor webhook or API payloads, look up the field in the vendor docs for the pinned API version, since fields such as a nested list can be omitted by default in newer versions), every helper cited for reuse (read its implementation to confirm sync vs async behavior, URL/argument matchers, and parsers actually support the new caller), checks the plan expects to fail (read the ignore lists, such as `deny.toml`), every cited `file:line`, and whether each live check can observe what it claims (same data root it installs to and reads from). A stale claim found here is a Blocker for the go decision.

The audit is done when the last stage has no open Blocker or Major finding. Set the design `Status: Approved` only when the user says so. Compress the audit turns; keep the design path, report paths and every accepted risk verbatim. Next stage: `ember-slice`.

## Consolidated mode

When the user said "run all": spawn stages 1 to 4 together in one `tools.parallel` call (stage 1 feeds only stage 5), then merge every finding into `audit/consolidated.md`, grouped as Blocker, Major and Nit, with duplicates merged by root cause and a table that maps each raw finding id to its merged item. A merged item takes the highest severity among its raw findings; a raw finding that names two causes maps to both items, noted under the table. Before leaving Resolve, verify with a quick script that every raw finding id across stages 1 to 4 appears in the mapping table and every merged item is classified in `audit/resolution.md`. Then resolve as above.

## Credits

Stages, personas, standards and the resolution classes come from Google's Elephant-Goldfish Model, Apache-2.0, https://github.com/google/elephant-goldfish-model at commit 72d06f52a2482885d9a64bd24b291c6e8628942c. `agents/`, `resources/document_standards/`, `LICENSE` and `NOTICE` are unmodified copies. This `SKILL.md` is a modified, condensed rewrite of `egm-review`, `egm-review-loop` and `egm-resolve` for Emberloop.
