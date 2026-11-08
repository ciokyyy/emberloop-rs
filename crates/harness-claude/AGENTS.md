# AGENTS.md — crates/harness-claude

## Purpose

Claude Code harness: runs the real `claude` binary over stream-json (subscription traffic never touches the proxy, so the proxy cannot meter it) and maps its events to `HarnessEvent`. Each `assistant` line carries one request's `message.usage`, emitted live as `HarnessEvent::Usage`; the meter reads that, never the `result` total. The proxy's context breakdown is absent for Claude: it never sees the request body.
- `ClaudeSession::ensure` starts the CLI with `--resume <token>` when the Session has one, and keeps the CLI's own session id from `system/init` as the resume token. The CLI emits `system/init` only after its first user message, so `write` sends first and then waits for init (`INIT_TIMEOUT`). A stdout EOF flags the process gone, so the next send respawns it even before `try_wait` sees the exit.
- The CLI runs with `--replay-user-messages`: it echoes each message when it takes it. A message sent mid-turn is folded into the running turn at a tool boundary (one `result`) or, in a text-only turn, waits and runs as a second turn (two). The session counts written-but-not-echoed messages and drops a `result` while any waits, so `TurnFinished` is the end of all the work, in both cases.
- Credentials: `sync_account` runs before every spawn and respawn (in `ensure`) and at every `TurnFinished` (before it is sent). The Account's and the Session's `.credentials.json` are compared by `claudeAiOauth.expiresAt` and the later one is copied over the other; the CLI rotates refresh tokens, so a stale copy is revoked. Copies are atomic and 0600. `.claude.json` is only seeded when the Session has none.
- The child env (`CLI_DISABLING_ENV`) always disables memory, CLAUDE.md files, git instructions, skills, compaction, telemetry, auto-update and background features, because the proxy owns the context.
- stderr is piped and its last line kept; a failed start or a broken pipe reports it (`with_tail`, `STDERR_DRAIN` bounds the wait).
- A Session whose `el` MCP server did not connect is refused. The permission mode is `dontAsk` with an explicit `--allowedTools` list.
- `fake/fake_claude.rs` (`fake-claude`) drives `tests/claude.rs` through `FAKE_CLAUDE_MODE` (`crash-once`, `delayed-init`, `stderr-no-init`, `no-init`, `mcp-failed`) and logs its args, config dir and `ANTHROPIC_BASE_URL` to `FAKE_CLAUDE_LOG`. `queue:` makes the fake finish the turn before it reads the queued message, as the real CLI does; `wait:` folds it into the turn. `tests/live.rs` needs a logged-in `claude` and a running proxy at `LIVE_PROXY_URL`; it is `#[ignore]` and not part of the check.

## Child DOX Index

(none)
