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

## Separate stored history from live process state

`ps -eo pid,ppid,lstart,args` lists the harness (`.../pi/v<ver>/pi --mode acp`) and its `emberloop-daemon mcp` sidecar per Session. Map a process to its Session from `/proc/<pid>/environ` (`EMBERLOOP_SESSION_URL=.../api/sessions/<id>`, `HOME=.../pi-homes/<id>`).

If the stuck Session's harness start time is later than the failures, and that process now answers a fresh prompt correctly with the same stored history, the fault was live process state (harness or proxy in-memory pair/fold/carry state), not the stored conversation. Confirm that before proposing any change; resuming the Session is then the fix, and editing history is not.

Pitfall: `journalctl -u emberloop` prints host local time while the daemon's tracing timestamps are UTC. Build grep windows from event epochs and shift them by the offset, or the window returns nothing.
