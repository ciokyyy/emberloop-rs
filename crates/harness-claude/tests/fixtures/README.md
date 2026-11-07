# harness-claude fixtures

`claude` 2.1.319, recorded 2026-11-07 against the flags the harness passes.

Real, then trimmed to the fields the parser reads (uuids, paths and prompt
templates removed; no content changed):

- `init-default-tools.json`: `system/init` from a run without `--tools ""`.
  The deny-list test reads its `tools` array.
- `init-mcp-failed.json`: `system/init` with `--tools ""` and an unreachable
  `el` MCP server; the harness refuses this Session.
- `auth-failed.jsonl`: a not-logged-in turn: `assistant` with
  `error: authentication_failed`, then `result` with `is_error: true`.

Not recorded (no login here), shapes from the documented stream-json format:

- `init-connected.json`: `init-mcp-failed.json` with `el` `connected`.
- `turn.jsonl`: text and thinking deltas, one `tool_use`/`tool_result` round
  trip, an `api_retry` status line, and a final `result`.

Replacing these with real captures is the first step once a login exists; the
fixture tests then pin whichever shapes the CLI actually emits.
