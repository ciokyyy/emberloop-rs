# A turn that stops without answering

Symptom: the prompt is accepted, status goes `working` then `idle` within about a second, no assistant text arrives, and no `usage` event is written for that prompt (`status` carries `stopReason: "end_turn"` with `usage: null`). The request never reached a model, so the event journal has no cause in it.

## Where the provider error lives

The harness records it; the daemon does not. One file per harness process:

`~/.local/share/emberloop-rs/pi-homes/<session>/.pi/agent/sessions/--<cwd with / turned into ->--/<ISO>_<resumeToken>.jsonl`

`meta.json`'s `resumeToken` is the file name suffix. Print `message.role`, `stopReason`, `errorMessage` and `usage.totalTokens` per line in `execute` and read the tail:

- healthy turn: assistant `stopReason: toolUse` (or `stop`) with a `toolResult` after it;
- failed turn: assistant `content: []`, `stopReason: "error"`, `totalTokens: 0`, and every later user prompt repeats the same `errorMessage`.

A run of identical error rows means the turn is permanently stuck for that process, not that the model is slow.

## Name the hop from the error shape

| `errorMessage` starts with | producer |
|---|---|
| `invalid request body:` | the Rust proxy; the body never parsed |
| `{"error":{"message":"Invalid value: ..."` | chatgpt codex backend, structured validation |
| `{"detail":"..."}` | chatgpt codex backend, terse rejection (`{"detail":"Store must be set to false"}`, `{"detail":"Bad Request"}`) |
| `Provider HTTP 5xx:` | proxy wrapping an upstream failure |

A bare `{"detail":"Bad Request"}` names no cause. Do not invent one from it; separate credential, guard and body faults instead (below).

## Prove the credential and the model

`.pi/agent/models.json` in the harness home gives `providers.<name>.baseUrl`, a `http://127.0.0.1:3000/proxy/<key>/x/<provider>/backend-api` route. `.pi/agent/auth.json` gives `<provider>.access` and its `expires` (milliseconds).

POST `<baseUrl>/codex/responses` with `Authorization: Bearer <access>` and

```json
{"model":"<model>","stream":true,"store":false,"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}]}
```

`store` must be `false`; leaving it out answers `{"detail":"Store must be set to false"}`. A 200 streaming body here proves the token, the model id and the proxy route are all fine, which leaves the conversation body as the only suspect.

Pitfall: this route goes through the proxy's own guards. A replay can come back `502 proxy tool round limit reached` for reasons that have nothing to do with the body under test; read that as a guard, never as the original fault.

## A request that hangs at the harness timeout

Symptom in the harness file: an assistant row with `content: []`, `stopReason: "error"`, `totalTokens: 0`, `durationMs` about 300000, and `errorMessage` `error sending request for url (http://127.0.0.1:3000/proxy/<key>/x/<provider>/backend-api/codex/responses): operation timed out`. The harness's own request timeout fired, so the proxy never produced a body the harness could read; nothing reached a model and no `usage` row exists for the attempt.

- Do not read it as a credential, model-id or body fault. Replay the same route once with the token from `auth.json` and measure the wall time: a streaming 200 in about a second proves route, token and model are all fine.
- A replay that succeeds while the Session's next `usage` row lands at the same second names a stalled upstream that recovered. The fix is to resume or re-prompt, not to change config.
- The harness retries a few times, and the daemon nudges `submit` after every dead turn, so each attempt waits the full timeout again: one 30-minute provider stall costs far more than 30 minutes of wall time.
- A subagent tier (`subagent_tiers.{smol,mid,top}`) whose ladder holds a single rung has nothing to escalate to, unlike reviewer ladders. Check rung count before trusting that tier during a provider stall.

## Separate stored history from live process state

`ps -eo pid,ppid,lstart,args` lists the harness (`.../pi/v<ver>/pi --mode acp`) and its `emberloop-daemon mcp` sidecar per Session. Map a process to its Session from `/proc/<pid>/environ` (`EMBERLOOP_SESSION_URL=.../api/sessions/<id>`, `HOME=.../pi-homes/<id>`).

If the stuck Session's harness start time is later than the failures, and that process now answers a fresh prompt correctly with the same stored history, the fault was live process state (harness or proxy in-memory pair/fold/carry state), not the stored conversation. Confirm that before proposing any change; resuming the Session is then the fix, and editing history is not.

Pitfall: `journalctl -u emberloop` prints host local time while the daemon's tracing timestamps are UTC. Build grep windows from event epochs and shift them by the offset, or the window returns nothing.

## agy (Antigravity) Sessions

agy keeps its own record; `meta.json`'s `resumeToken` names it under `~/.config/emberloop-rs/accounts/agy/default/antigravity-acp/`:

- `brain/<resumeToken>/.system_generated/logs/transcript.jsonl` — one JSON row per harness step, compact; `transcript_full.jsonl` is the same with untruncated fields.
- `brain/<resumeToken>/mcp/<server>/<tool>.json` — the MCP tool schemas this Session loaded. A missing `<tool>.json` means the tools server never exposed that tool here, which is not the same as the model refusing to call it.
- `conversations/<resumeToken>.db` — sqlite; no `sqlite3` binary is installed, so query it with Python's `sqlite3` in `execute`.

Read `transcript.jsonl` first:

- `USER_INPUT` rows are the daemon's prompts and steers.
- `PLANNER_RESPONSE` rows carry `input_tokens`/`output_tokens` and no content: the model answered, and the harness discarded the answer.
- `ERROR_MESSAGE` rows carry the harness's verdict plus its own retry counter (`Retries remaining: 4` down to `1`). A repeating response/error block means the harness resent the identical request about five times before the turn ended.

The raw request and the tool names the agent declared are in `gen_metadata`, one protobuf blob per step. Recover its printable runs:

```python
import sqlite3, re
db = "/root/.config/emberloop-rs/accounts/agy/default/antigravity-acp/conversations/<resumeToken>.db"
blob = sqlite3.connect(db).execute(
    "select data from gen_metadata order by idx desc limit 1"
).fetchone()[0]
print(*re.findall(r"[\x20-\x7e\n]{6,}", blob.decode("utf-8", "replace")), sep="\n")
```

Search the output for `mcp_` names and the model URL (it also proves which wire the turn used, e.g. `...:streamGenerateContent?alt=sse`).

Pitfall: agy registers eager MCP tools as `mcp_<server>_<tool>` with single underscores, where other harnesses send `mcp__<server>__<tool>`. Any proxy code that matches a tool name against a role allowlist must accept both spellings; `crates/api/src/status.rs` `normalize_tool_name` already does, `crates/proxy/src/wire.rs` `tool_name` strips only `__` nesting. When it misses, the model receives an empty `tools` array, and a harness message such as `encountered an improper format stop reason.\nPlease retry with a properly formatted function call.` is the model answering a request that tells it to call tools it cannot see. A daemon nudge that orders a `submit` on top of that keeps the loop alive until it is cancelled.
