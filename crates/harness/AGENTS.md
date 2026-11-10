# AGENTS.md — crates/harness

## Purpose

Defines the generic `Harness` and `HarnessSession` traits, harness wiring config, and common harness events. The proxy meters usage per request for every harness, so no harness emits usage events. `HarnessSession::forget(resume_token)` deletes the harness's own Session copy when the User deletes a Session; the agy driver sends it as an ACP `DeleteSessionRequest`. A Session survives a daemon restart through `HarnessWiring { resume_token, workdir }`: the harness loads the stored conversation in the same stable `workdir` (without replaying its history as events) and reports `resume_token()` and `resumed()`; `resumed() == false` means the conversation started over.

`HarnessSession::close()` stops the Session's harness process and waits for it to exit; the conversation stays resumable. The default is a no-op, and dropping a session also ends its process, so `close` is the path that waits. Each implementation owns its child: Claude kills and waits, ACP ends its connection and the SDK reaps the agent.

`HarnessSession::configure_model(model, effort)` changes a model and optional reasoning effort on the existing conversation and awaits acknowledgment; unsupported implementations return an error.

## Child DOX Index

(none)
