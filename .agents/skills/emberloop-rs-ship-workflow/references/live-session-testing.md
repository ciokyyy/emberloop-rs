# Live testing a Session on the running daemon

Use when a change must be proven against a real model, not only unit tests.

## Recipe

1. Create a test Session: `POST /api/sessions` with `projectId`, `title`, `model`, `thoughtLevel`. Leave it in place after the test and name it so the user can find it.
2. Send the prompt: `POST /api/sessions/{id}/prompt` with `{"text": ..., "images": []}`.
3. Steer: `POST /api/sessions/{id}/steer` with `{"text": ..., "images": [{"mimeType": ..., "dataBase64": ...}]}`. A 200 means accepted, not delivered.
4. Read the result from `GET /api/sessions/{id}/history?before=999999&limit=N` (JSON object/array of events with `seq`, `kind`, `data`). Never fetch `/api/sessions/{id}/events` with a plain blocking HTTP request expecting JSON: it is an open SSE stream and hangs until timeout.

## Check the steer route

- `steer_queued` then `steer_delivered` carry the steer `id`. `steer_delivered.data.route` says where it went: `sent at idle` means the turn had already ended and the steer started a new turn.
- A steer sent during a tool-running turn should show `steer_queued` then `steer_delivered` with `route: injected`. Read history right after the steer returns and before the turn ends: the history response may return only a recent window of `seq` values, and older steer events fall outside it.
- Do not claim the mid-turn path works from the model's reply alone. Probe with a pixel colour the prompt does not name and that is not the model's default guess (not red). A model that answers a common colour may be guessing. If no steer event shows the route, say the route is unverified.

## A tool the Session itself calls

- The Session that resumes after the update restart runs on the new daemon, and its tool declarations come from that daemon's MCP server, so a new argument is advertised without a harness restart: run the tool once with it set, read the tool result, the log it names, and a second call proving the state persisted (a stored key, a written file). No test Session and no HTTP route needed.
- A background job's exit notice reads `exited (code unknown)`, because the notice path only ever sees a vanished pid; `background` `wait` carries the real exit code. Judge success by `wait` and the log, not by the notice text.
- Pass the new argument explicitly: the call that omits it exercises the old path and proves nothing.

## Mechanics

- Write request bodies to `/tmp` files and send with `curl --data @file`. Inline JSON with base64 gets mangled through shell quoting.
- Large `curl` output is stored as an overflow handle. Write responses with `-o /tmp/file` and parse the file.
- Poll history with a bounded loop and a few seconds between reads. Do not hammer it with repeated long sleeps.
- Stop when the last `status` event is `state: idle` and the expected marker text appears in an `assistant_chunk`.
