# AGENTS.md — crates/tools

## Purpose

MCP tool server and tool implementations served to every harness over MCP.
`emberloop-mcp --root <project> --spill <spill dir>` serves rmcp over stdio;
`McpServer` implements `rmcp::ServerHandler` and is covered by tests. Tools,
path jail, spill store and seen-line store:

- `jail`: canonicalizes the Project root and refuses resolved paths outside it, symlinks included.
- `read`: plain text, numbered lines, 2 MiB cap, `[path#TAG] (lines A-B)`; directories list; `ctx:<handle>:<range>` pages spilled output.
- `grep`: regex over `ignore`'s `.gitignore`-aware walk; content, files or count output.
- `edit`: hashline grammar (`PUT N.=M:`, `PUT <N:`, `PUT >N:`, `PUT >$:`, `CUT N`, `CUT N.=M`), one-pass apply against original numbering, sha256 tag check, stale tag refused with current lines, anchors must have been shown by the tag's view.
- `bash`: `sh -c` in the root, own process group, timeout kill, 1 MiB output cap, `cat`/`head`/`tail`/`sed -n`/`grep -n` views feed the seen store.
- `background`: `list`, `read` (next byte offset), `wait`, `stop`; log files live in `spill/`.
- `overflow`: rendered output over `overflow_threshold` is written to `<data root>/sessions/<id>/spill/<handle>` and replaced by a head/tail summary naming `ctx:<handle>`.

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
