# Live testing a Session on the running daemon

Use when a change must be proven against a real model, not only unit tests.

## Recipe

1. Create a test Session: `POST /api/sessions` with `projectId`, `title`, `model`, `thoughtLevel`. Leave it in place after the test and name it so the user can find it.
2. Send the prompt: `POST /api/sessions/{id}/prompt` with `{"text": ..., "images": []}`.
3. Steer: `POST /api/sessions/{id}/steer` with `{"text": ..., "images": [{"mimeType": ..., "dataBase64": ...}]}`. A 200 means accepted, not delivered.
4. Read the result from `GET /api/sessions/{id}/history` (array of events with `seq`, `kind`, `data`).

## Check the steer route

- `steer_queued` then `steer_delivered` carry the steer `id`. `steer_delivered.data.route` says where it went: `sent at idle` means the turn had already ended and the steer started a new turn.
- A steer sent during a tool-running turn may produce no `steer_queued` or `steer_delivered` event in history. Then the route is unknown from history. Do not claim the mid-turn path works from the model's reply alone. Confirm with a probe that only the image can answer (e.g. a pixel colour the prompt does not name), and say the route is unverified if no event shows it.

## Mechanics

- Write request bodies to `/tmp` files and send with `curl --data @file`. Inline JSON with base64 gets mangled through shell quoting.
- Large `curl` output is stored as an overflow handle. Write responses with `-o /tmp/file` and parse the file.
- Poll history with a bounded loop and a few seconds between reads. Do not hammer it with repeated long sleeps.
- Stop when the last `status` event is `state: idle` and the expected marker text appears in an `assistant_chunk`.
