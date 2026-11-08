# Session retrospective and compression audit

## Scope and evidence

Session: https://agent.koboscintilla.com/p/proj_e4b40d5f94a3/s/ses_ed34fc53489b

The analysis uses `/root/.local/share/emberloop-rs/sessions/ses_ed34fc53489b/events/000000.jsonl` (2,676,425 bytes), its metadata, and the current Rust implementation. The recorded model is `opencode-go/deepseek-v4.1-flash`, with high thought level. Current source explains possible mechanisms, not necessarily the exact historical binary. There are no provider or MCP subspan traces.

## Where the time went

| Measurement | Seconds |
| --- | ---: |
| Total wall time | 3082.463 |
| Working-status intervals, union | 2455.564 |
| Tool execution envelopes, union | 706.438 |
| Remaining working time | 1749.126 |
| Idle/user wait | 626.899 |

There are 84 distinct tool executions, not 2,156. ACP publishes incremental input updates as repeated `tool_started` events. Each tool was measured from its first start to its finish. Input streaming between the first and last start totals only 5.474 seconds.

| Tool | Calls | Sum, seconds | Median, seconds | Maximum, seconds |
| --- | ---: | ---: | ---: | ---: |
| bash | 60 | 655.462 | 0.1255 | 173.960 |
| execute | 14 | 50.694 | 0.2515 | 47.433 |
| edit | 4 | 0.459 | 0.0885 | 0.252 |
| read | 5 | 0.214 | 0.0360 | 0.080 |
| write | 1 | 0.035 | 0.0350 | 0.035 |

The long calls are checks and builds: `scripts/check` took 173.960 seconds; a combined Svelte/Vitest command took 100.334 seconds; full Vitest took 67.467 seconds; another focused Vitest/Svelte check took 66.172 seconds; the web build took 65.242 seconds. One Vitest run reported 67.41 seconds overall but only 8.89 seconds executing tests. Transformation, collection, and environment setup matter; parallel timing categories are not additive.

Ordinary file tools are fast. Repeated checks and builds dominate measured tool time. Some commands used npm/npx despite the current Bun contract. These traces do not isolate the cost of npx itself.

The remaining 29 minutes of working time cannot be assigned entirely to harness overhead. It includes model generation, provider/network delay, retry waits, and orchestration gaps. Recorded retries include 2/4-second and 2/4/8-second waits. Usage totals are 3,841,649 input tokens, 28,929 output tokens, and 3,697,664 cached tokens. Input totals count repeated context, not unique text.

## Why edit was avoided

The old `execute` description explicitly recommended Python source replacements for multiple sites and included an `open(..., 'w')` example. Twelve execute calls wrote source or documentation through asserted string replacement before the user insisted on hashline edits.

All four `edit` calls occurred after that complaint. The first failed with `unexpected line "PUT 4:"; use PUT N.=M:, PUT <N:, PUT >N:, PUT >$: or CUT N`. The next call succeeded. Therefore, an edit failure cannot explain the earlier avoidance. Three successful edits took no more than 252 milliseconds each.

The earlier claim that bash reads cannot provide anchors was also too broad. Supported cat/head/tail/sed/grep forms populate seen lines; arbitrary pipelines do not necessarily return a tagged view. The prompt now explains valid syntax and recovery instead of encouraging a replacement-script fallback.

Several original commands piped checks into tail/grep and lost the failing command's exit status. Updated examples capture the status before printing a short log and return that status.

## Codebase memory and MCP startup

The original session only called `tools.search("graph")`. It made no `search_graph`, `trace_path`, or `get_code_snippet` queries. This is non-use, not evidence that the graph was useless.

Discovery took 47.433 seconds. `execute` currently resolves configured MCP tools before running the Python script; the registry connects servers and runs setup on first use. Cold MCP startup/index setup is a plausible contributor. The event log cannot separate package startup, indexing, transport, and server work. No lazy-connection redesign was made.

During this audit, real graph queries found `ctxsearch.run` and its dispatcher/test callers. Short `ctxsearch::run` tracing failed; passing the exact qualified name from search succeeded. Graph results help with symbols and static callers, less with markup and dynamic dispatch. No result does not prove absence.

Python attribute aliases such as `tools.search_graph(...)` work when unique. String dispatch through `tools.call` and `tools.parallel` requires `graph.search_graph`, not bare `search_graph`. The revised skill uses a bounded real example, exact qualified names, on-demand help, and explicit static-index limits. It replaces 86 lines of duplicated schemas with 19 lines of operational guidance.

## Changes

- `crates/prompts/prompts/policy.md`: shared hashline-edit workflow, failure-status preservation, focused checks before full checks, and distinct conversation versus spill search tools.
- `crates/prompts/prompts/tool.execute.md`: removes raw source-writing guidance; retains batching, filtering, and MCP discovery with correct dispatch names.
- `crates/prompts/prompts/tool.edit.md`: valid patch example, syntax recovery, and supported bash anchors.
- `crates/prompts/prompts/skill.codebase-memory.md`: bounded graph use and current-source verification.
- `crates/tools/src/ctxsearch.rs`: BM25 ranking over the existing spill chunks. The implementation uses k1=1.2, b=0.75, clamped IDF, OR matching, deduplicated case-insensitive word tokens, length normalization, and stable ranking. Existing handles and source validation remain unchanged.
- `crates/proxy/src/compressor.rs` and `server.rs`: durable block loading and safe conversation reset, described below.
- `crates/api/src/routes.rs`: `/api/version` uses the build version when supplied, matching the daemon CLI. The old endpoint incorrectly reported Cargo's 0.0.12 while release 0.0.21 was running.
- Owning `AGENTS.md` files document these contracts. Root `AGENTS.md` documents the local update entry point.

The Go reference was read only at `/tmp/emberloop-go/emberloop-web/el/tools/ctxsearch.go` and `/tmp/emberloop-go/emberloop-web/el/statedb/ctx.go`. Rust ports the core BM25 behavior, not the entire Go retrieval subsystem. It still uses 20-line chunks, has no Porter stemming, heading weighting, structural chunking, or persistent FTS5 index, and scans spill files per search. No dependency or data migration was added.

## Compression persistence: reproduced data-loss bug

Previously, `NativeCompressor::new` always started with no blocks and next id `b1`. Files survived a daemon update, but block search, replay, and restore state did not reload. A later fold could reuse `b1` and overwrite saved originals. A regression test reproduced this before the fix.

The constructor now reloads `blocks/*.json`, validates ids and filenames, restores exact summary turns and restored flags, and continues after the largest saved id. Unreadable or malformed committed files fail runtime creation visibly rather than being overwritten. Block writes use a temporary file and rename. Decompression persists the restored flag before changing the in-memory conversation.

A new harness conversation archives the old directory as `blocks-reset-N`, then starts an empty compressor. Old originals remain available on disk instead of being silently replaced by restarted ids.

New-format nested folds replay byte-exactly after reopening. Legacy files preserve originals but never stored exact summary markers or restored flags; reconstructed markers cannot guarantee all legacy nested chains match byte-exactly. Writes do not include fsync and do not guarantee power-loss durability.

No compression/decompression calls were found in the original audited session. The initial live-data scan found no existing block directories. This independently reproduced bug is real, but cannot be claimed as the cause of the original session's symptoms.

## What persistence does not solve

- Original turns still remain in RAM in `Vec<Block>`. Matching/restoration clones some values; status serializes originals for token estimates.
- The runtime map retains every touched Session without eviction. Durable reload is not a bounded-memory implementation.
- No memory/CPU profile was run. A daemon snapshot of 100,504 KiB RSS and 58 MB of Session files cannot attribute memory use to compression.
- Harness continuity depends on accepting the saved resume token. ACP load failure starts a new conversation; the runner then resets proxy history. The UI event log is not automatically reconstructed into model context.
- Prompt pinning lasts for a live runtime. A daemon restart pins current prompt text again; saved hashes alone cannot recover old text.
- `tools.store` is persisted in `execute_store.json`, not merely RAM. Its current flush is a direct write with ignored write errors, not an atomic durable transaction. This separate reliability limit was not changed. Saved execute scripts also live on disk.

Disk-backed lazy originals, runtime eviction, startup subspan instrumentation, and stronger execute-store writes require separate targeted work. No real-harness restart experiment was performed.

## Tests and deployment

Test-first changes include the prompt guidance regression, two BM25 regressions, compression restart recovery, nested replay, legacy/corrupt block handling, conversation-reset archival, and build-version reporting. The BM25 and prompt regressions failed before their fixes. Compression restart recovery failed before its fix. Build-version reporting failed with `EMBERLOOP_BUILD_VERSION=0.0.999` before the route fix and passed afterward.

Focused results: 17 prompt tests, four ctx-search tests, and 99 proxy library tests passed. Workspace clippy passed. A full check exposed two more issues: the MCP test compared serialized JSON key order, and a stale embedded web bundle failed its freshness test. The MCP test now compares parsed JSON and passes; the bundle was rebuilt with Bun. The check script also piped nextest into tee without pipefail, hiding failing tests. It now preserves the nextest failure status. Earlier stage success alone was not proof that every test passed. The final full check log is `/tmp/emberloop-retro-final-check.log`. The generated-file gate still requires the pre-existing changes in `openapi.json` and `web/src/lib/api/schema.d.ts` to be committed; those unrelated changes were not reverted or committed. Final workspace nextest run: all 382 tests passed. The full check passed formatting, clippy, dependency checks, doc tests, and generation; its only failure is the generated-file cleanliness gate. The web stage reused unchanged inputs after an explicit successful `(cd web && bun run build)`.

Deployment has not been queued. The installed release at inspection was 0.0.21; the next local release is 0.0.22. The release symlink and `/proc/85685/exe` confirmed 0.0.21 despite the old API reporting 0.0.12. Run `scripts/dev-update` only after resolving the check gate. Inside a Session it queues an install and safe restart at a turn end when all Sessions are idle. A queued request is not proof of installation. After restart, compare the current release CLI version, API version, and running process executable.

## Primary sources

- [Writing effective tools for agents](https://www.anthropic.com/engineering/writing-tools-for-agents): clear tool contracts, examples, and trajectory-based evaluation.
- [Code execution with MCP](https://www.anthropic.com/engineering/code-execution-with-mcp): progressive discovery, batching, and filtering. Its example token savings are not measurements of this project.
- [SQLite FTS5 BM25](https://sqlite.org/fts5.html#the_bm25_function): scoring reference for the Go implementation and this port's core formula.
- [LangGraph persistence](https://docs.langchain.com/oss/python/langgraph/persistence): distinction between durable conversation checkpoints and other stores; no dependency adopted.
