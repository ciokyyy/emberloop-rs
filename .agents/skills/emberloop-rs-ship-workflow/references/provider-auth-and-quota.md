# Diagnosing provider auth and quota on a live daemon

Use when a Session idles, refuses to resume, or one agent 401s while another works on the same account.

## Order

1. Rule out account state before reading code. An account at its subscription usage limit leaves a Session `idle` with no failed event and no daemon error; the user usually knows. Ask once, then stop if they confirm.
2. Compare credentials across Session homes. Each Session holds a copy under `<data root>/claude-homes/<session id>/`; one copy's CLI rotates its own OAuth grant and Anthropic revokes the siblings, which produces `401 authentication_error ... OAuth access token has been revoked` in one agent only. Read `claudeAiOauth.expiresAt` in each home's `.credentials.json`.
3. Do not treat `GET /api/providers` `signedIn: true` as a health check: it only proves a token file exists, and a revoked token still reads signed in.

## Recovery path

- A completed login writes into the account home and spreads to every Session home (`harness_claude::sync_account`, `claude_login::spread`), so revoked agents heal on their next turn; a Session whose CLI already 401'd may need one more turn or a restart to read it.
- A login started in a page that was then reloaded must not wedge the backend: `claude_login::start()` adopts a running unexpired flow (10 minute lifetime) and returns it instead of `409 a Claude login is already running`.
- Cover both with `crates/daemon/tests/claude_login.rs`: a fake `#!/bin/sh` CLI writing `{"claudeAiOauth":{...}}` to `$CLAUDE_CONFIG_DIR/.credentials.json` proves adoption and fan-out without real credentials.

## Mechanics

- Read `<data root>` paths (`sessions/<id>/meta.json`, home dirs, the `current` symlink) with `cat`/`head` from bash; the `read` tool resolves only inside the project root and refuses them.
- Check the running version before debugging a symptom the fix already covers: `readlink <data root>/current` and `curl localhost:3000/api/version`.
