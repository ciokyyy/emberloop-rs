# AGENTS.md — crates/tools

## Purpose

MCP tool server and tool implementations served to every harness over MCP.
`emberloop-mcp --root <project> --spill <spill dir>` serves rmcp over stdio;
`McpServer` implements `rmcp::ServerHandler` and is covered by tests. Descriptions
are the Prompt Store's `tool.<name>.description` texts: `McpServer::new` advertises
the embedded defaults and the daemon overrides them with `with_descriptions`. Input
schemas are inline in `mcp.rs`; `tests/contract.rs` checks each text against its
schema. Tools, path jail, spill store and seen-line store:

- `jail`: canonicalizes the Project root and refuses resolved paths outside it, symlinks included.
- `read`: plain text, numbered lines, 2 MiB cap, `[path#TAG] (lines A-B)`; directories list; `ctx:<handle>:<range>` pages spilled output. An `http(s)://` or `www.` path is fetched instead of read from disk (`readurl`, no separate tool): direct HTTP, HTML-to-Markdown, JSON pretty-print, Jina reader fallback, loopback SSRF guard.
- `grep`: regex over `ignore`'s `.gitignore`-aware walk; content, files or count output.
- `find`: lexical code finder with `search`, `outline`, `architecture`, and `trace` modes; search hits feed the seen-line store.
- `edit`: hashline grammar (`PUT N.=M:`, `PUT <N:`, `PUT >N:`, `PUT >$:`, `CUT N`, `CUT N.=M`), one-pass apply against original numbering, sha256 tag check, stale-tag rebase when changes miss anchored lines, anchors must have been shown by the tag's view.
- `write`: writes a complete file inside the Project root, creating parent directories and recording the snapshot in the seen store.
- `websearch`: searches Brave and Exa APIs with freshness, domain filters, and 15-minute file cache; keys come from the tool-key file Settings writes (`ToolConfig::tool_keys_file`, read on every call), never from the environment.
- `execute`: runs `python3 -I` in its own process group with a generated `tools.py` Unix-socket bridge (`tools.parallel`, `tools.store`/`load`, `tools.save`/`run`, 500-call cap). It is the only way to an MCP server: `mcpclient.rs` loads `<config root>/mcp.json` (Claude shape, `include`/`exclude`/`enabled`/`summary`/`project`/`setup`; a `url` entry is refused), connects each stdio server with rmcp on first use, and exposes every tool as `tools.<server>.<tool>(...)` plus a bare alias when one server owns that name. `graph.rs` names the codebase-memory server and resolves its `project` argument (the entry's `project`, then `list_projects` matched by root path, then the path with separators as dashes), so a model never passes it.
- `bash`: `sh -c` in the root, own process group, `EMBERLOOP_SESSION_DIR` exported (the Session directory), the runtime env file sourced first, timeout kill, 1 MiB output cap, `cat`/`head`/`tail`/`sed -n`/`grep -n` views feed the seen store.
- `background`: `list`, `read` (next byte offset), `wait`, `stop`; log files live in `spill/`; sources the same runtime env file.
- `intercept`: the gate in front of `bash` (`intercept.rs`, first match wins, `EL_BASH_INTERCEPT=off` disables it). A command that belongs to another tool is refused before anything runs and the reply names that tool plus `Command not run: <line>`: `sed -i`/`perl -i`/`awk inplace` -> `edit`, inline Python (`-c`, stdin, heredoc) -> `execute`, `cat`/`echo`/`printf` with a file redirect and `tee` -> `write`, `nohup` and a trailing `&` -> `background: true`. It is routing, not security: a quoted `>` and `2>` pass, `python3 file.py` and `python3 -m http.server` stay allowed, and reads are never refused, so their views still feed the seen-line store `edit` checks. A refused command returns before the run and before `feed_views`, so it marks nothing seen.
- Runtime env: every shell sources `<config root>/env` (the daemon passes it; standalone falls back to `${XDG_CONFIG_HOME:-$HOME/.config}/emberloop-rs/env`) when that file exists, so editing it changes the next command with no rebuild and no daemon restart. `ToolConfig::env_file` overrides the path for tests.
- `todo`: the plan as one ordered list addressed by stable ids (`t1`, `t2`, …), one tool with `action` of `read`, `set`, `add`, `update`, `remove` or `clear`; statuses `pending`, `in_progress`, `completed`, `cancelled`, at most one in progress; ids never reused, `set` restarts them; persisted as `todos.json` in the Session directory (temp name plus rename).
- `ctx_search`: BM25-ranked word matches over the Session's spill chunks, with OR semantics, case folding, punctuation splitting, term-frequency saturation and length normalization; each hit has a pageable `ctx:<id>:<start>-<end>`. It scans existing spill files without a migration or new dependency. Unlike Go's FTS5 store, it has no Porter stemming, heading weighting, or persistent index.
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
