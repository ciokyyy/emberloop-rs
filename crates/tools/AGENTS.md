# AGENTS.md — crates/tools

## Purpose

MCP tool server and tool implementations served to every harness over MCP.
`emberloop-mcp --root <project> --spill <spill dir>` serves rmcp over stdio;
`McpServer` implements `rmcp::ServerHandler` and is covered by tests. Tools,
path jail, spill store and seen-line store:

- `jail`: canonicalizes the Project root and refuses resolved paths outside it, symlinks included.
- `read`: plain text, numbered lines, 2 MiB cap, `[path#TAG] (lines A-B)`; directories list; `ctx:<handle>:<range>` pages spilled output. An `http(s)://` or `www.` path is fetched instead of read from disk (`readurl`, no separate tool): direct HTTP, HTML-to-Markdown, JSON pretty-print, Jina reader fallback, loopback SSRF guard.
- `grep`: regex over `ignore`'s `.gitignore`-aware walk; content, files or count output.
- `find`: lexical code finder with `search`, `outline`, `architecture`, and `trace` modes; search hits feed the seen-line store.
- `edit`: hashline grammar (`PUT N.=M:`, `PUT <N:`, `PUT >N:`, `PUT >$:`, `CUT N`, `CUT N.=M`), one-pass apply against original numbering, sha256 tag check, stale-tag rebase when changes miss anchored lines, anchors must have been shown by the tag's view.
- `write`: writes a complete file inside the Project root, creating parent directories and recording the snapshot in the seen store.
- `websearch`: searches Brave and Exa APIs with freshness, domain filters, and 15-minute file cache.
- `execute`: runs `python3 -I` in its own process group with a generated `tools.py` Unix-socket bridge (`tools.parallel`, `tools.store`/`load`, `tools.save`/`run`, 500-call cap).
- `bash`: `sh -c` in the root, own process group, `EMBERLOOP_SESSION_DIR` exported (the Session directory), timeout kill, 1 MiB output cap, `cat`/`head`/`tail`/`sed -n`/`grep -n` views feed the seen store.
- `background`: `list`, `read` (next byte offset), `wait`, `stop`; log files live in `spill/`.
- `git`: read-only inspection (diff, log, show, blame, status, ls-files, grep, rev-parse, merge-base, cat-file, branch/tag listing); writing, executing and configuring flags are refused by prefix, config environment cleared.
- `check`: runs a named check from `<root>/.emberloop/check` (plain command or JSON name-to-command map with one `{args}` slot, shell metacharacters refused); answers a pass/FAIL/TIMED OUT verdict plus the last 60 lines.
- `todo`: the plan as one ordered list addressed by stable ids (`t1`, `t2`, …), one tool with `action` of `read`, `set`, `add`, `update`, `remove` or `clear`; statuses `pending`, `in_progress`, `completed`, `cancelled`, at most one in progress; ids never reused, `set` restarts them; persisted as `todos.json` in the Session directory (temp name plus rename).
- `ctx_search`: term-scored snippets over the Session's spill files, each with a pageable `ctx:<id>:<start>-<end>`.
- `overflow`: rendered output over `overflow_threshold` is written to `<data root>/sessions/<id>/spill/<handle>` and replaced by a head/tail summary naming `ctx:<handle>`. Handle lookup accepts the proxy's `c`-prefixed spelling and the bare one interchangeably.
## Limits

Seen-line snapshots persist in the Session's `seen/` directory (`state.json`,
atomic rename). A snapshot stores the full sha256 beside the 4-hex display tag,
and `edit` compares the full hash, so a 4-hex collision cannot pass a stale edit.

Background records persist `<id>.json` (pid, `/proc/<pid>/stat` start time, boot
id) next to `<id>.log`. A new `Toolbox` (daemon restart) adopts a process group
only when pid, start time and boot id all match; otherwise the handle is `lost`
and `stop` never kills an unknown process. An adopted exit reports
`exited (code unknown)`; an exit that happened while the daemon was down emits
its notice exactly once. `bash` is not a sandbox: the jail constrains tool path
arguments only.


(none)
