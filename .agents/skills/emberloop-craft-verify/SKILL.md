---
name: emberloop-craft-verify
description: Use when checking which Craft apps work as Emberloop MCP tools, or when enabling a passing one in a Session's Project.
metadata:
  origin: agent
---

# Verify and enable a Craft app

`scripts/verify-craft [app...]` reads `scripts/verify-craft.tsv` (`app<TAB>cli binary<TAB>web bundle URL`) and prints one markdown table row per app. Each row runs two checks:

- MCP: spawns `<cli> mcp`, sends `initialize` then `tools/list` over stdio, and passes when at least one tool arrives within 20 s.
- Web: downloads the release bundle, unzips it, serves it with `python3 -m http.server`, and passes when `index.html` answers 200 and a `*.wasm` file starts with the `\0asm` magic bytes.

Usage:

1. Run `scripts/verify-craft` for every app, or name apps to check only those (`scripts/verify-craft figcraft`).
2. Read the table: `pass`, `fail` or `skip` in the MCP and Web columns, with the reason in the last column (`19 tools, wasm-ok`, `no-cli`, `no-web`, `wasm-magic`, `index-404`). A `skip` reason is the curl error, so retry before judging the app.
3. The script exits 1 when any cell failed or a named app is not in the table. An app with no web asset (anicraft) has an empty URL, so its Web column is `fail` with reason `no-web`.
4. Run `scripts/verify-craft --selftest` after changing the script: it builds a fake CLI and two bundles, then asserts a pass row, a `wasm-magic` fail row and a download `skip`.

## Enable a passing app

A `fail` row stays off; do not install or enable it. For a `pass` row:

1. Add a gated entry to `<config root>/mcp.json`, keyed by the tool name the Project will list:

   ```json
   "photocraft": { "command": "photocraft-cli", "args": ["mcp"], "gated": true }
   ```

2. Add the tool with that name in the Project drawer (or `PATCH /api/projects`). Gating hides the server from every Project that does not list an enabled tool of the same name; `"enabled": false` hides a tool from all Sessions. Changes reach new Sessions only.

FilmCraft stays out: video editing is a skill, not an MCP server. A remote Craft server is an entry with `url` and `headers` instead of `command`; see the `mcp.json` section of `README.md`.
