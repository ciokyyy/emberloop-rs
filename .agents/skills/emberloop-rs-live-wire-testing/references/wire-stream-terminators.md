# Wire stream terminators

What an answer the proxy writes itself (no provider call, hand-built body) must contain so each
harness parser accepts it. The parsers live in the pi fork (`crates/pi-ai/src/providers/<wire>/events.rs`,
`.../google/stream.rs`); each one raises in `end()` when its terminator is absent.

## Anthropic (`anthropic/events.rs`)
- `event:` lines are required: the handler returns early when the event name is not one of
  `message_start`, `message_delta`, `message_stop` or a `content_block_*`.
- Minimal accepted stream: `message_start` (message with `id`, `model`, `role: assistant`,
  `content: []`, `usage`), `message_delta` with `delta.stop_reason: "end_turn"`, `message_stop`.
- Missing terminator strings: `Anthropic stream ended before message_stop`,
  `Anthropic stream ended without a stop reason`.

## OpenAI Chat Completions (`openai_completions/events.rs`)
- `end()` errors `Stream ended without finish_reason` unless some chunk carried a non-empty
  `choices[0].finish_reason`. Emit `data: {...,"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":"stop"}]}`
  then `data: [DONE]` (the `[DONE]` line is not required by `end()`, only by convention).

## OpenAI Responses (`openai_responses/events.rs`)
- Only a terminal `response.completed` (or `response.incomplete`) ends the stream: `finalize` reads
  `data["response"]`, where `status: "completed"` maps to a normal stop. Error otherwise:
  `OpenAI Responses stream ended before a terminal response event`.

## Google (`google/stream.rs`)
- `end()` errors `{label} stream ended without a finish reason`; the chunk must carry a candidate
  with `finishReason: "STOP"`.
- Shape depends on the route: pi sends `/v1beta/models/<model>:streamGenerateContent` and reads
  top-level `candidates`; the Cloud Code route (`v1internal:streamGenerateContent`) expects the
  payload nested as `{"response": {...}}`. Decide from the request itself (`request["request"]`
  present means Cloud Code nesting), not from the path alone.

## Both forms
- Streaming vs buffered: Anthropic/Chat/Responses carry `stream` in the body; Google has no such
  field, so use the path (`:streamGenerateContent`) or `alt=sse` in the query.
- Buffered answers are the same payload without SSE framing, with `content-type: application/json`.
- An empty assistant message is safe for the pi agent loop: it special-cases only
  `StopReason::Error` and `StopReason::Aborted`, and a message with no tool calls ends the turn.
