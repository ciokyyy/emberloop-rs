---
name: ember-build
description: Use when implementing a ticket, fixing a bug or making any code change. Runs the per-ticket loop in one long-lived Session - capture the starting point, test first, find the root cause before any fix, prove every claim with fresh command output - then hands the diff to ember-review.
metadata:
  origin: agent
---

# Ember build

Three laws hold for every change:

1. No production code without a failing test first.
2. No fix without a root cause first.
3. No claim of "done", "fixed" or "passing" without fresh command output that shows it.

## The ticket loop

For each ticket, lowest unblocked number first:

1. Start. Record the ticket's absolute path. Capture the starting point before any edit: `git rev-parse HEAD` (no commits yet: use the empty tree `4b825dc642cb6eb9a060e54bf8d69288fbee4904`). Record pre-existing dirty files from `git status --short`; they are not part of this ticket. Set the ticket `Status: in progress`.
2. Build it test first (below). Change only what the ticket needs: minimum code that solves it, no speculative abstractions, configurability or error handling for impossible scenarios, and no dead code "for the next ticket". Keep edits surgical: do not reformat, refactor or rewrite comments on untouched code; remove only imports, variables or helpers orphaned by your edit; report pre-existing dead code instead of deleting it unless the ticket asks.
3. Update the docs the ticket lists, including the nearest `AGENTS.md`.
4. Verify (below): focused tests, then the repository's full check command once.
5. Review: run `ember-review` with the ticket path and starting point. Pause all edits and builds until both reports arrive.
6. Commit only when the user authorizes it and the checks pass. Stage files by name; never `git add -A`. Set the ticket `Status: done`.
7. Compress the ticket's turns. Keep verbatim: ticket path, starting point, commit SHA, check command and result, any open finding. Then take the next ticket.

The Session stays alive across tickets. It remembers why each decision was made; `compress` keeps it lean. Do not start a new Session per ticket.

## Test first

- Red: write one test for one behaviour, with a name that states the behaviour. Use real code; mock only what you cannot run.
- A changed default or config value: test the behaviour it drives, read through the production mapping (for example the private settings-to-policy function, reached from the module's own test child), not `Default::default().x == N`. An equality test only restates the number; the behaviour test proves the value reaches the code that uses it, and still fails before the edit.
- Verify red: run it. It must fail for the expected reason, not from a typo, a compile error or a missing fixture. A test that passes at once proves nothing; fix the test.
- Green: write the least code that passes it.
- Verify green: run the test, then the suites the change can reach. Report every failure by name, even one you think is unrelated.
- Refactor with tests green, then run them again.

Code written before its test is deleted and rewritten test first. Skip the test only when the user agrees: throwaway prototypes, generated code, pure config. Hard to test means the design needs a seam; say so instead of skipping. For a bug, the first test reproduces the bug and fails.

Good tests (details in `references/writing-good-tests.md`): name the production change that would make the test fail; derive expected values independently (never recompute them with code under test); assert behaviour, not that a mock was called; keep test-only code out of production; know a dependency's side effects before you mock it.

## Debugging

Use this for any bug, failing test or surprise, before you change code. Redact any secret as `<REDACTED>` in commands, logs and outputs you show.

1. Feedback loop first. Build a tight red signal before reading code to theorize: a failing test, curl/HTTP probe, CLI snapshot diff, headless browser script, replayed trace, fuzz loop or bisection harness. Watch it fail with the exact symptom reported.
2. Investigate and minimize. Read the whole error, stack trace and recent changes (`git log`, `git diff`). Shrink the repro until every remaining input, config and step is load-bearing. Log what enters and leaves boundaries with a unique prefix (`[DEBUG-<id>]`) so one `grep` deletes all probes later, and trace the bad value backwards (`references/root-cause-tracing.md`). For performance regressions, measure a baseline and bisect instead of logging.
3. Compare and hypothesize. Compare against similar working code and list every difference. Generate 3-5 ranked falsifiable hypotheses ("if X is the cause, changing Y does Z") before testing any; show the list when the user is present so domain knowledge can re-rank it. Test one variable at a time.
4. Fix. If a regression test would sit on too shallow a seam, flag the missing seam instead of trusting a false-confidence test. Otherwise turn the minimized repro into a failing test at the real call-site seam, make one fix at the root cause, and verify against the original un-minimized scenario. Add checks at each layer the bad value crossed when that prevents a repeat (`references/defense-in-depth.md`). Replace sleeps and timeouts in tests with waits on the real condition (`references/condition-based-waiting.md`). Find a test that pollutes shared state with `references/find-polluter.sh`.
5. Clean up. Confirm every `[DEBUG-<id>]` probe and throwaway harness is removed, and state the confirmed root cause in the commit or report.

After three failed fixes, stop. The architecture is the likely problem; bring the evidence to the user before a fourth attempt. Red flags that mean go back to step 1: theorizing before a red loop exists, "quick fix for now", several changes at once, a fix you cannot explain.

## Verify before you claim

Before any claim of status: name the command that proves it, run it fresh and in full, read the output and the exit status, then state the result with the evidence.

| Claim | Needs |
| --- | --- |
| Tests pass | Test output with 0 failures, this run |
| Build or lint clean | Exit status 0, this run |
| Bug fixed | The test of the original symptom passes |
| Regression test works | It failed without the fix and passes with it |
| Subagent finished | You read the diff yourself; its report is not evidence |
| Ticket done | Each acceptance criterion checked line by line |

Words such as "should", "probably" or "seems to" mean you have not verified. Run the command.

A check that fails only on its total time budget while every stage reports green is a cold-build overrun: rerun it once warm and report the overrun. Never raise, skip or soften the budget.

## Delegating to workers

Write the code yourself by default. Use workers when the user asks for it, or for disjoint tickets that can run at once.

- Tier `mid` unless the user allows `top`. One owner per file or ticket; two workers never edit the same file.
- The worker reads the ticket file and this skill; its brief names the ticket path, starting point, files it owns, the focused check command, and "end with a submit call".
- Keep a worker alive across tickets with `steer`; one long-lived worker running tickets in sequence beats a new spawn per ticket.
- Workers run scoped checks; you run the full check once, after their diffs land. Serialize heavy builds if the repository has a lock (for example `flock /tmp/<repo>-build.lock <command>`).
- Read every worker diff before review. A worker report that says "done" proves nothing.

## Credits

The test-first, debugging and verification rules come from obra/superpowers, MIT, copyright Jesse Vincent, https://github.com/obra/superpowers at commit 22f0b46. The `references/` files and `references/LICENSE-superpowers` are unmodified copies. The ticket loop, tight debugging loop and tautological-test rule draw from Matt Pocock's `implement`, `diagnosing-bugs` and `tdd` skills; surgical-change and simplicity rules draw from `ForrestChang/andrej-karpathy-skills` (MIT), rewritten for Emberloop.
