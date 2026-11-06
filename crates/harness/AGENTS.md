# AGENTS.md — crates/harness

## Purpose

Defines the generic `Harness` and `HarnessSession` traits, harness wiring config, and common harness events. `HarnessSession::forget(resume_token)` deletes the harness's own Session copy when the User deletes a Session; the agy driver sends it as an ACP `DeleteSessionRequest`. A Session survives a daemon restart through `HarnessWiring { resume_token, workdir }`: the harness loads the stored conversation in the same stable `workdir` (without replaying its history as events) and reports `resume_token()` and `resumed()`; `resumed() == false` means the conversation started over.

`HarnessSession::configure_model(model, effort)` changes a model and optional reasoning effort on the existing conversation and awaits acknowledgment; unsupported implementations return an error.

## Child DOX Index

(none)
