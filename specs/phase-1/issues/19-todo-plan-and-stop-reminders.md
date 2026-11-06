# Ticket 19: Todo plan, one `todo` tool, plan card, stop reminders

## Goal

One `todo(action, …)` tool over MCP with stable ids and a `cancelled` status. The daemon publishes the list as Session state, the SPA shows it as a plan card above the composer, and the runner sends a capped stop reminder through the steer queue when a turn ends with open items. Blocked by tickets 05, 09, 10.

Design mockup: https://claude.ai/artifact/6dHRkB1gSALWViEXf52HMj (private to the author; the decisions below are the contract).

## Status

Done.

## Decisions

- **One tool, one action argument.** `todo` takes `action` of `read`, `set`, `add`, `update`, `remove` or `clear`. No second tool, no `todo_read`.
  - `read`: no arguments, returns the full list with ids and counts.
  - `set`: `items: [{content, status?}]`, replaces the list and assigns ids `t1…tN`; returns the full list.
  - `add`: `items`, optional `after: id`; existing ids never change; returns one line.
  - `update`: `updates: [{id, status?, content?}]`, batch patch; returns one line such as `t2 completed · t3 in_progress · 2 left · next: t4 "…"`.
  - `remove`: `ids`. `clear`: empties the list.
- **Statuses:** `pending`, `in_progress`, `completed`, `cancelled`; the existing aliases keep parsing. At most one `in_progress`; a violation, a duplicate content string or an unknown id is a tool error whose message says the fix in one line.
- **Ids are stable and never reused** within a list's life; a `set` restarts them. Persistence stays `todos.json` in the Session directory (temp name plus rename).
- **Reminders are the daemon's decision and the steer queue's delivery.** The proxy never reads `todos.json`. The runner enqueues a `Notice` on turn end; Route 2 (idle wake) delivers it. The reminder text comes from the one policy source, never from a harness.
- **A reminder stays in the Session history** (steers are saved as splices and replayed unchanged). Keep the text to one line plus the open ids.
- **Open means `pending` or `in_progress`.** `completed` and `cancelled` never trigger a reminder.
- **Settings** (Session-independent, in Settings → Todo, stored with the other daemon settings):

  | key | default | meaning |
  |---|---|---|
  | `todo.reminders` | `true` | stop reminders on or off |
  | `todo.remindersMax` | `3` | per prompt cycle, reset by the next user message; `0` is off |
  | `todo.autoExpand` | `true` | plan card opens for 2 s on create or replace |
  | `todo.hideWhenDone` | `true` | card fades a few seconds after the last item resolves |

## Reminder rules

A reminder is enqueued only when all hold at turn end:

1. `todo.reminders` is on and fewer than `remindersMax` were sent this prompt cycle.
2. At least one item is open.
3. The open set (ids and statuses) differs from the set at the last reminder this cycle.
4. The Session is settled: no steer queued, no background job running, no question pending for the user.

Never mid-turn. A reminder ends the cycle's count only by reaching the cap or by the open set emptying. The cap resets when the user sends a message. When the cap is reached once, the feed shows a dashed note saying so and the runner sends nothing further.

## Tests first

Failing before implementation, in order:

**Tool (`crates/tools`)**
- `set` assigns `t1…tN`, `add` keeps existing ids, `update` patches by id and a batch of two (finish one, start next) is one call.
- Two `in_progress` is refused with a fix hint; duplicate content and an unknown id are refused; `cancelled` parses and counts as resolved.
- `update` and `add` return one line; `set` and `read` return the full list; `clear` removes the file; an empty `set` behaves like `clear`.
- Ids are not reused after `remove`.
- The MCP schema lists `action` as an enum and the description names every action; the prompt file `tool.todo.md` is updated and the prompts test that lists tool prompts passes.

**Reminders (`crates/daemon`, fake clock, no real sleep)**
- Turn end with an open item enqueues one `Notice` and the idle wake delivers it as one merged prompt with any queued steer.
- Nothing is enqueued when: reminders are off, the list is empty, every item is resolved, the open set is unchanged since the last reminder, a steer is queued, a background job runs.
- The cap stops the count at `remindersMax`; a user message resets it; `remindersMax = 0` sends none.
- The reminder is replayed byte-identically on later requests (reuses the ticket 09 check).
- Settings change takes effect on the next turn end without a restart.
- Open question to settle by test, not guess: how the runner detects "a question is pending for the user". If the harness exposes nothing, the fallback is that a queued steer or running background job counts as not settled, and the gap is written into the owning `AGENTS.md`.

**API and events (`crates/api`)**
- A `todos` event carries the full list on every change; the Session snapshot includes the current list; both are in the OpenAPI document and the regenerated client; a reload restores the list.
- The settings keys read and write through the existing settings routes.

**Web (`web/`)**
- Plan card is hidden with no open item, collapsed by default to `done/total`, the active item and a progress bar; click and the `p` key toggle it.
- Expanded view caps at 8 rows with `+N more` and repeats the active item when it falls outside the visible rows; statuses render as `[ ]`, `[~]`, `[x]`, `[–]` with completed and cancelled struck through.
- Auto-expands for 2 s on create or replace when `todo.autoExpand` is on, then collapses; fades after the last item resolves when `todo.hideWhenDone` is on.
- The `todo` tool row in the feed is one line (`update · t2 completed · 2 left`), never the full list.
- The reminder shows as a dashed note with `Reminder n of max`; the cap note shows once.
- The Settings → Todo panel writes the four keys.
- Fake timers for every delay; no real sleep.

## Out of scope

Dependencies between items, cross-Session or shared lists, subagent lists (phase 2), a `/todos` command, a mid-turn stale nudge (add only after real Sessions show it is needed).

## Docs to update in the same change

`crates/tools/AGENTS.md` (the `todo` line), `crates/daemon/AGENTS.md` (turn-end reminder), `crates/api/AGENTS.md` if present, `web/AGENTS.md` (plan card), `specs/phase-1/issues/AGENTS.md` (range), and the Go-reference note if behaviour differs.
