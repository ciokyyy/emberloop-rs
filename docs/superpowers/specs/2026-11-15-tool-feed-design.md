# Tool Feed Dots Redesign & Full-Width Composer Spec

**Date:** 2026-11-15
**Design source:** `design.figcraft` page `Tool feed · Astryx dark` (`Feed · quiet · dots`) and `design.md`

## Goals

1. Make `execute` tool calls immediately legible without opening them (record real inner tool calls from the bridge; surface intent from ACP/MCP metadata, first meaningful Python comment, or child call summary).
2. Always start every tool row—including the last row—closed by default (failed rows stay open with `#2a2222` tint and error tail).
3. Surface Antigravity (`agy`) intent (`toolAction`, `toolSummary`, ACP `call.title`) as the row title and `WorkingIndicator` active label.
4. Clean up thinking rows across models (skip finished empty thinking blocks; show a one-line pulse while an empty live thinking span streams; use first sentence as collapsed header).
5. Adopt the quiet "dots" visual grammar (`5px` dot in an `8px` box, `32px` desktop / `44px` mobile rows, always-visible durations, dropped `"Ran"`/`"Execute"` verb words, Lucide icons kept for file extensions on read/write/edit and app chrome) and expand the composer + plan/working wrapper to full width to match the chat feed.
6. Fix misclassified `el` tools (`submit`, `stop`, `result`, `prompt_propose`, `retry_call`, `ctx_search`, `acp_status`) so they never display as `"Ran"`.

## Backend Changes

1. **`crates/harness/src/lib.rs` & `crates/harness-acp/src/lib.rs`:**
   - Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub title: Option<String>` to `HarnessEvent::ToolStarted`.
   - Forward `call.title` (and `update.fields.title`) when non-empty and distinct from `name`.

2. **`crates/tools/src/execute.rs`:**
   - Track inner tool calls in `BridgeState::children: Mutex<Vec<ExecChild>>` inside `serve_conn` (`{ name, target, ms, status }`).
   - Persist `<session_dir>/exec/<exec_id>.children.json` when `run_in_bridge` completes (exit or timeout).

3. **`crates/daemon/src/tooldisplay.rs` & `crates/daemon/src/runner.rs`:**
   - Pass `title: Option<&str>` into `ToolTracker::on_start`. Extract `toolAction` / `toolSummary` from `input` (top-level or `arguments`/`Arguments`) before stripping, combine with ACP `title`, and emit `display.title` (merged with `classify_bash_display` when applicable). Store `start_display` on `TrackedTool`.
   - Pass `&output` into `ToolTracker::on_finish`. Start from `start_display` so `ToolFinished` retains `title`, `kind`, and `parts`; merge `diff` for write/edit tools; for `execute`, parse `saved exec #<id>` from `captured_output` or `output` and load `<session_dir>/exec/<id>.children.json` into `display.children`.

## Web UI Changes

1. **Full-width composer (`web/src/lib/components/Composer.svelte`, `web/src/routes/+layout.svelte`):**
   - Replace `mx-auto max-w-[760px]` with `w-full` on Composer's inner container and the `+layout.svelte` PlanCard/WorkingIndicator wrapper.

2. **Feed state (`web/src/lib/feed.ts`, `web/src/lib/toolRuns.ts`, `web/src/routes/+layout.svelte`):**
   - Merge `display` objects across `tool_started` and `tool_finished` instead of replacing `row.display`.
   - Track `startedAt`/`elapsedMs` on `thinking` items when closing live spans.
   - Populate `ToolRun.children` from `item.display?.children` in `runOf()`.
   - Pass `display.title` (falling back to `shortTool`) into `WorkingIndicator` via `activeTool`.

3. **Tool classification & compact model (`web/src/lib/toolfeed.ts`, `web/src/lib/compactFeed.ts`):**
   - Classify `submit`, `stop`, `result`, `prompt_propose`, `retry_call`, `acp_status` cleanly; never route `submit` to `ran`.
   - Compute human-readable row `title` + optional mono `target` chip: `display.title` > meaningful Python comment (`# ...`) / child summary / static `tools.<name>` call summary > first non-import line.
   - Skip empty finished thinking runs; mark empty live thinking runs as pulse rows (`empty: true, live: true`); extract `firstSentence` and `lineCount` for non-empty thinking rows.
   - Format always-visible duration for all completed calls (`<1s` or `Ns`).

4. **Dots rendering (`web/src/lib/components/ToolFeed.svelte`):**
   - Remove `LAST_OPEN_ROW`; all rows (including running and the last row) start closed; failed rows (`status === 'failed'` or non-zero exit) start open with `#2a2222` tint.
   - Render `32px` (`44px` mobile) rows with an `8px` box holding a `5px` dot (`fg-4` idle, `status-working` pulsing when running, `status-failed` when failed), `13px` `fg-2` title (`fg-3` for groups/thinking), optional mono target chip, `FileIcon` preserved for read/edit file rows, diff stats, duration, and `'›'` chevron.
   - Expanded `execute` card shows Calls (`name`, `target`, `ms`, status dot), Code, and Output.
