# Goldfish Support (Elephant-Goldfish Model) Design

**Date:** 2026-11-16
**Upstream:** https://github.com/google/elephant-goldfish-model (EGM, Apache-2.0, v0.1.1)

## Intent

EGM is a set of skills plus reviewer personas. The main Session (the elephant) holds project
context and drafts a design. Reviewers (goldfish) run in fresh contexts and see only the document
and the code, so they expose assumptions the elephant no longer sees. Emberloop should run EGM
well: the goldfish guarantee must hold in code, not only in a prompt, and EGM must install without
editing its files.

## Goals

1. A new `spawn` role, `goldfish`, that starts a fresh, read-only, one-shot child Session whose
   system prompt is a persona loaded from a `.md` file.
2. Personas are plain files with `name` and `description` frontmatter, the format EGM, Claude Code
   and omp already use. Any user can add one.
3. A goldfish Session carries no history, no instruction files, no style prompts and no skills
   catalog. This is emberloop's equivalent of Claude's `omitClaudeMd`.
4. EGM installs unmodified as one directory under the global skills root, plus one small
   emberloop index skill and one `docs/emberloop.md`.

## Non-goals

- Plugin manifests (`plugin.json`, marketplaces, `owner/repo` install). Only EGM is supported.
- A daemon-run review loop, review "Runs", or a findings UI. The elephant LLM drives the loop
  through the EGM skills, like every other emberloop skill flow.
- A persona editor in the web UI.
- Claude-specific adapters (`.claude-plugin/agents/`). Emberloop does its own isolation.
- Nested subagents. `egm-synthesizer` is not used; the elephant dispatches directly, the path
  `docs/harnesses.md` upstream already documents for hosts that cannot nest.

## Anti-goals

- A goldfish that can see the parent conversation, in any way. That is a silent failure of the
  whole feature.
- A persona file able to widen its own access (`tools:` frontmatter must be ignored).

## Approaches considered

1. **Skill-driven, enforced `goldfish` role (chosen).** The elephant runs EGM skills; the daemon
   guarantees isolation and persona placement. Small Rust change, EGM text stays diffable.
2. **Daemon-driven review runs.** An endpoint runs the five stages and stores findings. Best UX,
   but a large build, it reverses the existing "no Runs" decision for reviewers, and it forks EGM
   logic into Rust.
3. **Scout plus "read the persona file first"** (what Codex does). Zero daemon work, but the
   persona is user-turn text, not the system prompt, and nothing removes Ponytail, Caveman,
   skills or instruction files from the child. Rejected: it is not goldfish isolation.

## Design

### Persona files

- Format: `*.md` with YAML frontmatter `name` and `description`. The body is the system prompt.
  Other frontmatter keys (`tools`, `model`, `license`) are ignored.
- The persona id is the frontmatter `name` (for example `egm-zoom-out`), not the file name.
- Search order, first match wins; scope beats form, so a project persona always shadows a global
  one, bundled or standalone:
  1. `<project root>/.agents/agents/*.md`
  2. `agents/*.md` inside each skill folder under the project skill root
  3. the directory beside the global skills root, `agents/*.md` (`~/.agents/agents`)
  4. `agents/*.md` inside each skill folder under the global skills root (this is how EGM's
     personas are found: `~/.agents/skills/egm/agents/`).
- Discovery reads files at spawn time; no cache and no restart. A file without a valid `name`
  is skipped with a `tracing::warn!`, like a bad skill.
- New `GET /api/personas?project=<id>`: read-only list of `{name, description, path}`. No editor.

### `spawn role=goldfish`

- Input: `persona` (required id), `text` (the task: mode, absolute target path, absolute
  standards paths), optional `title`, optional `tier` (default `mid`).
- Unknown persona: `400` naming the persona and listing the available ids.
- Fresh Session per call. A goldfish is stateless and one-shot: every entry point that can send it
  a turn returns `400` (the subagent `steer` action, `/api/sessions/{id}/prompt` and
  `/api/sessions/{id}/steer`). The check lives once, in the shared daemon prompt/steer handler,
  keyed on role. The web composer is hidden for the goldfish role. Follow-up goes to the elephant,
  never back into the same reviewer.
- 1-level rule applies: a goldfish cannot call `spawn`, `steer`, `stop` or `result`.
- Reports go through `submit` and the existing delivery path. Default title
  `Goldfish (<persona>)`. The sidebar shows a goldfish badge. The `ReviewerAssignment` role is
  `goldfish`, with the persona id stored for the badge and recovery.
- Recovery and `watch_submits` nudge treat `goldfish` like `scout`.
- **Goldfish ladders.** New setting `goldfish_tiers` (`smol|mid|top`, same `SubagentTiers` shape
  as `subagent_tiers`, edited in Settings like the other ladders). A goldfish `spawn tier=X` walks
  `goldfish_tiers.X`; an empty goldfish ladder falls back to the matching `subagent_tiers` ladder.
  Purpose: run reviewers on a model family different from the elephant's, so blind spots are not
  shared. Emberloop cannot enforce "different family"; the Settings help text says so. Goldfish
  are not forced to differ from each other. Diversity between parallel reviewers comes from the
  tier hint per persona (`top` for design critic and implementation readiness, `mid` for the
  rest), not from rotating models.
- After its report is delivered, the daemon stops the goldfish through the existing stop path. The
  transcript stays readable and the sidebar row shows as stopped. A review of up to 15 reviewers
  must not leave 15 live Sessions. Recovery must not resurrect a stopped goldfish.

### Isolation (the core of the feature)

A goldfish request, built in `prepare_request` (`crates/proxy/src/server.rs`):

- **System prompt:** the persona body plus the proxy's tool-workflow policy only. Dropped for
  goldfish: Ponytail, Caveman, the superpowers pointer, the skills catalog, the codebase-memory
  prompt and the reviewer prompts. Today `inject_prompts_for_wire` appends all pinned prompts to
  every Session, scouts included, so this needs a goldfish-specific pin set.
- **Tools:** an exact allowlist of seven: `read`, `grep`, `find`, `websearch`, `ctx_search`,
  `todo`, `submit`. Everything else is absent, including `edit`, `write`, `bash`, `background`,
  `execute`, `skill_search`, `skill_manage`, `prompt_propose`, `retry_call` and the proxy-owned
  `compress`, `decompress`, `search_context`, `acp_status`. Reason: `bash` and `execute` can write,
  and a persona that says "strictly read-only" must be true. This is stricter than `scout`, which
  keeps `bash` and `execute`. Compression is off for a goldfish: it is one-shot and short, a fold
  would swap read evidence for the goldfish's own summary, and a review that nears the window
  should be split across more goldfish. The billion-context nudges are not sent. The proxy's
  automatic overflow fold stays as the safety net. If real reviews hit it often, add a higher
  per-role threshold then.
- **Instruction files:** the goldfish loads none. Claude Code runs with
  `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`. agy and pi qualify by construction: the working directory is
  the empty `<session>/agent-work`, not the Project; agy runs `enabledTools: []` with
  `GEMINI_HOME` set to emberloop's own account dir; pi runs under `env -i` with a per-Session
  `HOME`, `--no-builtin-tools`, and a generated config. No capability gate in code. Each harness
  ships a live-wire isolation probe (see Tests); if a harness fails it, add a refusal gate for
  that harness only.
- **Read jail:** a goldfish `read` goes through `ProjectJail::resolve` with an allowlist of three
  roots: the Project, the skill roots and the goldfish's own Session directory. Everything else is
  refused: other Sessions' directories, the parent transcript in `claude-homes/<id>` and
  `pi-homes/<id>`, other projects, `~/.ssh`, daemon settings. An allowlist, not a blocklist: a
  `sessions/`-only block would pass the isolation test and still expose the parent conversation.
  Consequence: task `text` may name only absolute paths inside the Project or a skill root.
- **Known limit:** the goldfish still sees the whole Project, including a committed `AGENTS.md`,
  if it chooses to `read` it. Upstream documents the same limit (issue #237). Stable conventions
  go in instruction files; assumptions to challenge go in the task `text`.

### EGM installation

- Copy the upstream repo unmodified to `~/.agents/skills/egm/`: `skills/`, `agents/`,
  `resources/`, `docs/`, `LICENSE`.
- Add `~/.agents/skills/egm/SKILL.md`, an emberloop index skill (like `superpowers`). It lists the
  seven `egm-*` skills, tells the elephant to read `docs/emberloop.md`, then the chosen
  `skills/<name>/SKILL.md`, and to follow it in the same thread. Only the index appears in the
  catalog, so upstream's `disable-model-invocation: true` needs no support.
- Add `docs/emberloop.md` (ours, new file). It states:
  - the plugin root is the folder holding `SKILL.md`; EGM's `${CLAUDE_PLUGIN_ROOT}`, `agents/`
    and `resources/` all resolve against it, and every path passed to a goldfish is absolute;
  - the mapping: "spawn a subagent with persona X" is `spawn role=goldfish persona=X`;
  - `egm-synthesizer` and `bootstrap_orchestrator` are not used (they nest); dispatch directly;
  - review stages run sequentially, with parallel goldfish only inside a stage; wait for every
    report before the next stage;
  - the `EGM artifact directory:` line is read from the project `AGENTS.md`;
  - tier hints for `spawn tier`: `top` for `egm-design-critic` and `egm-implementation-readiness`,
    `mid` for style, structural, comprehension and zoom-out. Do not inherit the spawner's exact
    model: correlated blind spots defeat a fresh review;
  - the pipeline with Matt Pocock's skills: `grill-with-docs` (decisions, GLOSSARY, ADRs), then
    `egm-draft`, `egm-review` and `egm-resolve` on the design doc, then `to-tickets`, then
    `implement-spec`. The reviewed design doc is the spec; `to-spec` is skipped or only publishes
    that doc. `egm-brainstorm` is optional after a grilling session. `implement` runs as a separate
    step after the user approves the doc, since EGM itself never writes production code.
- A path in an upstream skill that says to read `docs/claude.md` or `docs/codex.md` is overridden
  by the index skill, which says to read `docs/emberloop.md` instead.
- The directory is a repo-vendored copy with a NOTICE line (Apache-2.0, "not an officially
  supported Google product"). Where the copy lives in this repo (a `vendor/` path, a script that
  installs it, or a defaults bundle) is the builder's call; the Session loader needs it only
  under a skill root.

## Tests

- `crates/proxy`: a goldfish request carries the persona and no Ponytail, Caveman, superpowers,
  catalog or reviewer text. Its declared tools equal the exact allowlist above, on every wire shape
  `scout_filter_removes_write_across_all_tool_shapes` covers.
- `crates/api/tests/subagents.rs`: spawn unknown persona is `400`; steer is `400` through the
  subagent action, `/prompt` and `/steer`; nesting is blocked; default title; tier is accepted;
  fresh Session per call; recovery re-sends the persona and does not resurrect a stopped goldfish;
  a goldfish is stopped after `submit` delivery.
- Persona discovery: a project-bundled `foo.md` beats a global standalone `foo.md`, all four
  levels of the order, a skill-bundled persona found, a bad frontmatter skipped.
- Read jail: a goldfish `read` fails for the parent `events.jsonl`, for
  `claude-homes/<parent>/...`, for `pi-homes/<parent>/...` and for `~/.ssh`; it succeeds for the
  Project, a skill-root `resources/` file and its own spill.
- Per-harness isolation probe (claude, agy, pi; `emberloop-rs-live-wire-testing`): plant a marker
  in an `AGENTS.md`/`GEMINI.md` in `agent-work` and in the account or session home, spawn a
  goldfish, ask it to quote any instructions beyond its persona. The marker must not appear.
- A live-wire check: run `egm-review` on a real design doc and confirm each reviewer reports from
  a fresh Session.

## Docs and contracts to update

`crates/api/AGENTS.md`, `crates/prompts/prompts/tool.spawn.md` (the model-facing description),
`GLOSSARY.md` (Elephant, Goldfish), `openapi.json`, `web/src/lib/api/schema.d.ts`, and the
`emberloop-rs-tool-authoring` rules for the changed tool schema.

## Left to the builder

- The exact shape of the goldfish pin set in `prompts`/`proxy` and where the persona is placed.
- Where the read-jail check lives, and whether it also applies to scouts and workers (default: no).
- The vendoring location for EGM in this repo.
- Whether the pi build can drop context files by flag; the probe decides if that is needed.

## Risks

- Dropping `execute` also drops the codebase-memory graph tools, which sit behind it. A persona
  that wants "code search" has `grep` and `find` only. Acceptable for v1.
- Some personas may rely on tools or paths not yet read; only `zoom_out.md` has been read in full.
  The builder reads all ten before fixing the tool list.
- EGM is at v0.1.1 and changing fast; a vendored copy drifts. Record the upstream commit in the
  NOTICE.
