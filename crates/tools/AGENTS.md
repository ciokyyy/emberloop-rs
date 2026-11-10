# AGENTS.md — crates/tools

## Reviewer subagents

`subagents.rs` proxies `spawn`, `steer`, `stop`, `result` and `submit` to the current Session API when `ToolConfig.session_url` is set. `ToolConfig.reviewer` removes only `edit` from MCP declarations and rejects direct or nested execute dispatch to it. Other tools remain available; this is not a filesystem sandbox.


## Purpose

MCP tool server and tool implementations served to every harness over MCP.
`emberloop-mcp --root <project> --spill <spill dir>` serves rmcp over stdio;
`McpServer` implements `rmcp::ServerHandler` and is covered by tests. Descriptions
are the Prompt Store's `tool.<name>.description` texts: `McpServer::new` advertises
the embedded defaults and the daemon overrides them with `with_descriptions`. Input
schemas are inline in `mcp.rs`; `tests/contract.rs` checks each text against its
schema. Tools, path jail, spill store and seen-line store:

- `jail`: canonicalizes the Project root and refuses resolved paths outside it, symlinks included. `ProjectJail::with_extra_root` (set from `ToolConfig::global_skills`) also admits the canonical global skills root and the canonical target of each direct child of it, a symlinked skill, listed on every call. Writes (`resolve_writable`) stay inside the Project root.
- `read`: plain text, numbered lines, 2 MiB cap, `[path#TAG] (lines A-B)`; directories list; `ctx:<handle>:<range>` pages spilled output. An `http(s)://` or `www.` path is fetched instead of read from disk (`readurl`, no separate tool): direct HTTP, HTML-to-Markdown, JSON pretty-print, Jina reader fallback, loopback SSRF guard.
- `grep`: regex over `ignore`'s `.gitignore`-aware walk; content, files or count output.
- `find`: lexical code finder with `search`, `outline`, `architecture`, and `trace` modes; search hits feed the seen-line store.
- `edit`: hashline grammar (`PUT N.=M:`, `PUT <N:`, `PUT >N:`, `PUT >$:`, `CUT N`, `CUT N.=M`), one-pass apply against original numbering, sha256 tag check, stale-tag rebase when changes miss anchored lines, anchors must have been shown by the tag's view.
- `write`: writes a complete file inside the Project root, creating parent directories and recording the snapshot in the seen store.
- `websearch`: searches Brave and Exa APIs with freshness, domain filters, and 15-minute file cache; keys come from the tool-key file Settings writes (`ToolConfig::tool_keys_file`, read on every call), never from the environment.
- `execute`: runs `python3 -I` in its own process group with a generated `tools.py` Unix-socket bridge (`tools.parallel`, `tools.store`/`load`, `tools.save`/`run`, 500-call cap). It is the only way to an MCP server: `mcpclient.rs` loads `<config root>/mcp.json` (Claude shape, `include`/`exclude`/`enabled`/`summary`/`project`/`setup`; a `url` entry is refused), connects each stdio server with rmcp on first use, each in its own process group that is killed when the registry drops (a helper a server started does not outlive the Session's tool server), and exposes every tool as `tools.<server>.<tool>(...)` plus a bare alias when one server owns that name. `graph.rs` names the codebase-memory server and resolves its `project` argument (the entry's `project`, then `list_projects` matched by root path, then the path with separators as dashes), so a model never passes it. The daemon's `npx -y codebase-memory-mcp@<version>` graph entry runs the native `codebase-memory-mcp` instead when one is in a `ToolConfig::bin_dirs` directory (`PATH`, then `~/.local/bin`); npx is only the fallback. `setup` runs once per `CBM_CACHE_DIR`: a finished run writes `emberloop-setup.json` there with the setup it applied, and a later Session skips it while that record matches; a failed step reruns on the next connect. The marker is per cache dir, not per Project, because codebase-memory keeps its config in that dir.
- `figcraft.rs` creates `design.figcraft`; `mcpclient/design.rs` connects every project-scoped FigCraft MCP process to the same durable server document. Private edit links live under `FIGCRAFT_LIVE_DIR`, keyed by canonical project-root hash and created under a cross-process file lock. `FIGCRAFT_SERVER_DATA` identifies the server key directory. Calls pump collaboration before and after tools, including inspection and pointer tools; files are snapshots, not a synchronization transport. Missing live configuration fails instead of starting a detached editor. `tests/figcraft_live.rs` is an explicit native check for bidirectional edits, project isolation and server restart recovery.
- `bash`: `sh -c` in the root, own process group, `EMBERLOOP_SESSION_DIR` exported (the Session directory), the runtime env file sourced first, the group is killed on timeout and again once the shell exits (a detached child does not outlive the call; `background` is the only way to keep one), 1 MiB output cap, `cat`/`head`/`tail`/`sed -n`/`grep -n` views feed the seen store.
- Blocking waits (`bash` foreground `timeout`, `background wait`, `execute` `timeout`) go through `Toolbox::call_secs`/`cap_secs`: each tool's own maximum, lowered to `ToolConfig::max_call_secs` when set. The `mcp` subcommand reads it from `EMBERLOOP_MAX_CALL_SECS`, which the Antigravity harness sets to 170 because Antigravity cuts an MCP call at 3 minutes and then exits.
- `background`: `list`, `read` (next byte offset), `wait`, `stop`; log files live in `spill/`; sources the same runtime env file.
- `intercept`: the gate in front of `bash` (`intercept.rs`, first match wins, `EL_BASH_INTERCEPT=off` disables it). A command that belongs to another tool is refused before anything runs and the reply names that tool plus `Command not run: <line>`: `sed -i`/`perl -i`/`awk inplace` -> `edit`, inline Python (`-c`, stdin, heredoc) -> `execute`, `cat`/`echo`/`printf` with a file redirect and `tee` -> `write`, `nohup` and a trailing `&` -> `background: true`. It is routing, not security: a quoted `>` and `2>` pass, `python3 file.py` and `python3 -m http.server` stay allowed, and reads are never refused, so their views still feed the seen-line store `edit` checks. A refused command returns before the run and before `feed_views`, so it marks nothing seen.
- Runtime env: every shell sources `<config root>/env` (the daemon passes it; standalone falls back to `${XDG_CONFIG_HOME:-$HOME/.config}/emberloop-rs/env`) when that file exists, so editing it changes the next command with no rebuild and no daemon restart. `ToolConfig::env_file` overrides the path for tests.
- `todo`: the plan as one ordered list addressed by stable ids (`t1`, `t2`, …), one tool with `action` of `read`, `set`, `add`, `update`, `remove` or `clear`; statuses `pending`, `in_progress`, `completed`, `cancelled`, at most one in progress; ids never reused, `set` restarts them; persisted as `todos.json` in the Session directory (temp name plus rename).
- `ctx_search`: BM25-ranked word matches over the Session's spill chunks, with OR semantics, case folding, punctuation splitting, term-frequency saturation and length normalization; each hit has a pageable `ctx:<id>:<start>-<end>`. The scoring lives in `bm25.rs`, shared with `skill_search`. It scans existing spill files without a migration or new dependency. Unlike Go's FTS5 store, it has no Porter stemming, heading weighting, or persistent index.
- `overflow`: rendered output over `overflow_threshold` is written to `<data root>/sessions/<id>/spill/<handle>` and replaced by a head/tail summary naming `ctx:<handle>`. Handle lookup accepts the proxy's `c`-prefixed spelling and the bare one interchangeably.
- `skill_search`: BM25 over every discovered skill (name, description, body headings and the first 2 KB of body). `query` is required; `limit` defaults to 5, max 20. Each hit gives name, origin, the description clipped to 150 characters and the absolute SKILL.md path; `read` loads it.
- `skills.rs`: `discover(global, project)` lists the `SKILL.md` folders under the global root and under `<project>/.agents/skills`, following symlinked folders. Project skills come first; a project skill hides a global skill of the same name. A malformed skill (frontmatter without a valid `name` and `description`, or a folder name that differs from `name`) is skipped with a warning. `origin` comes from `metadata.origin`, default `human`. `catalog(intro, global, project)` renders the frozen catalog: the intro, then `- name: description` lines capped at 50 and `N more skills: use skill_search` beyond that, ordered by root and then by latest `view` in each root's `.usage.jsonl`.
- `skill_manage`: an `ops` array applied all or nothing: `create`, `patch`, `write_file`, `remove_file`; no delete in v1. Only `metadata.origin: agent` skills change, and `create` forces that origin. Every touched skill is locked (`<root>/.locks/<sha256(name)>.lock`, `File::lock`) across its read-modify-write. A patch or write to an existing file is refused unless the current bytes were read in this Session (`Read <path> with read before changing it.`). Each change first copies the old file to `<root>/.backup/<name>/<unix-seconds>-<file>` and keeps ten per skill. Supporting files sit under `references/`, `templates/`, `scripts/` or `assets/` and may not reach outside through a symlink. A failing op undoes the earlier ones in the call. Each call appends one line to `<root>/.usage.jsonl`.
- `prompt_propose` (`promptpropose.rs`): `id`, `text`, `reason`; stages a `prompts::ProposalStore` proposal for the User to approve in Settings and never changes the active prompt. `id` alone returns the active text (the review's `read` cannot reach the config root); `text` without `reason`, or the reverse, is an error. `ToolConfig::prompts_dir` (`<config root>/prompts`) says where; without it the call fails with `prompt_propose needs the daemon's prompts directory`. Declared to every Session like `skill_manage`, because the background review replays the Session's tool list.
- `skillstore.rs`: lock, backup, usage-log, `view` log and support-path helpers behind `skill_manage`. `read` of a `SKILL.md` directly under a skill root appends a `view` line. `bm25.rs`: the tokenizer and BM25 scorer shared by `ctx_search` and `skill_search`.
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
