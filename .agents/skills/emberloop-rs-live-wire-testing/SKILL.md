---
name: emberloop-rs-live-wire-testing
description: Use when testing emberloop-rs proxy wire behaviour (cross-wire message translation, model or harness switching) against real models. Covers the one-key opencode-go route to Anthropic, OpenAI Chat, and Responses models, and the translation rules the core turn format needs.
metadata:
  origin: agent
---

# Live wire testing

## Route
- One saved `opencode-go` key in `<config root>/keys.json` reaches all three wires. Read it inside `execute` Python, never print it. Base `https://opencode.ai/zen/go/v1`: `/chat/completions`, `/messages`, `/responses`. `GET /models` lists ids and the wire each takes.
- Every request needs header `x-opencode-session: <any id>`; without it the API answers 400 `MissingSessionID`. Anthropic wire also wants `anthropic-version` and `x-api-key`.
- Haiku rejects `thinking.type=enabled`; use `thinking:{type:adaptive}` plus `output_config:{effort}`.
- Upstream stream decoding error (`upstream error: error decoding response body`): reqwest yields this when the upstream server terminates the TCP/TLS connection mid-stream during SSE delivery or fails HTTP chunk/compression decoding. Investigate upstream stability or connection drops, not client SSE parser bugs.
- Call with `curl` from `execute` (stdlib only). Use absolute paths: `HOME` is a per-session dir.

## Translation via core turns (`crates/proxy/src/wire.rs`)
- Path: `extract_core_contents(src)`, sanitize, `write_core_contents(dst)`. Both are public.
- Sanitize must remove `_raw` from every turn: the writers emit `_raw` verbatim, so otherwise source-wire messages leak into the target wire.
- Drop `opaque` parts (thinking, signatures, unknown blocks): they hold only a digest, and the target cannot replay them. Drop empty text parts and turns left empty. Merge same-role neighbours.
- Verified live both ways: DeepSeek chat, Haiku (Anthropic), and gpt Responses accept each other's translated tool-call history. DeepSeek accepts a history with `reasoning_content` stripped.
- Not covered yet: images, Google wire, undeclared old tool names, ID format rules.

## ACP session config options
- Never hardcode a config option id. Read the ones the harness advertises in its `session/new` response and send a level under the advertised id; agy advertises `thinking_level` (`acp_server/config_options.py`: `MODEL_ID = "model"`, `MODE_ID = "mode"`, `THINKING_LEVEL_ID = "thinking_level"`), while other harnesses use `thought_level`, and pi's fake harness advertises only `model`.
- An unknown id is a hard error, not ignored: agy answers `Protocol error: Unknown config option id: thought_level: null` and the Session setup maps it to 409. So model-only changes must still succeed when no level id is advertised: set `model` first (propagate its error) and send the level best-effort.
- Discover ids without running the harness: the agy server is a zip/PEX at `<data root>/agy-acp/<version>/agy_acp_server.par`; unzip it and grep `config_options.py` and the `Unknown config option id` raise site. Its levels are `minimal, low, medium, high, xhigh, extra_high, max` and are not guaranteed to contain every level this repo's catalogue lists, which is another reason the level send must be best-effort.
- Build a model catalogue from `configOptions`, not from any `models` payload: `session/new` returns the `model` select, and `session/set_config_option` for each model returns the updated `configOptions` whose `thinking_level` or `thought_level` options are that model's accepted efforts. The pinned ACP schema's `NewSessionResponse` has no `models` field even when agy sends one, so a schema bump will not expose it. A model with no level picker accepts no effort; `xhigh` appears only for models that support it.
- Verify the wire shape with a unit test on the serialized request (`configId` and a string `value`, not null) and a live switch both ways on a real Session (PATCH `/api/sessions/<id>` with `model` + `thoughtLevel`), restoring the original pair afterwards.

## Capturing real harness histories
- Run a capture+forward HTTP proxy (script under `target/scratch/`, started with `bash background: true`; absolute paths) that saves each POST body and forwards upstream. Point `claude` at it with `ANTHROPIC_BASE_URL=http://127.0.0.1:<port>` (`claude -p --model haiku --permission-mode dontAsk --allowedTools Bash`). Point pi at it via a scratch `HOME` with `.pi/agent/models.json` (`{providers:{opencode-go:{baseUrl,apiKey}}}`, mode 0600) and `settings.json` `{"transport":"sse"}`; run `pi -p --no-session --offline --tools bash --model opencode-go/<id>`.
- The forwarder must add `x-opencode-session` for opencode-go upstream.
- pi appends `/v1/messages` itself: for Anthropic models set `baseUrl` without `/v1`, else 404 `/v1/v1/messages`; chat/responses models want `/v1`.
- The tool-result turn (last capture per run) is the useful history. Replay it translated to every other wire, declaring the source tool name (`Bash`/`bash`) in the target request.
- Verified: Claude Code, pi-deepseek, pi-gpt, pi-haiku histories all replay on all three wires (12/12 200).
- Cleanup: stop forwarders with the `background` tool `stop`, never `pkill -f` (it kills its own shell). Delete scratch HOME dirs: they hold the key.

## Driver
- Spike in `crates/proxy/tests/translate_spike.rs`: offline round-trip tests, plus an `#[ignore]` `cli` test (`TR_FROM`/`TR_TO` in chat|anthropic|responses, `TR_IN`, `TR_OUT`). Build with `cargo test -p proxy --test translate_spike --no-run`, run the newest `target/debug/deps/translate_spike-*` binary with `--ignored cli` from a Python loop.
- Live loop: native turn on model A with a tiny `add` tool, append the assistant message and tool result, translate, send to model B, check the answer.
