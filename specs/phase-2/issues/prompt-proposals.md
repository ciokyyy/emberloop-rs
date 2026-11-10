# Prompt proposals: the background review stages prompt edits, the User approves them

## Request and evidence

The background skill review (`review.skills`) can write agent skills but cannot change model-facing prompt text. Two agent skills (`hashline-editing`, `context-continuity`) held rules that belong in `tool.edit.description` and `compress.prompt`; a person moved them by hand. The User wants the review to propose such edits for later Sessions, with the User approving every one. A prompt text is pinned per Session, so an approved edit reaches new Sessions only and never invalidates a running Session's cache.

## Scope

The review fork stages a proposal for one prompt id. The User sees the current text beside the proposed text in Settings, then approves or rejects it. Approval writes the existing override through `PromptStore::set_override`. Rejection deletes the proposal.

In scope: prompt ids only (model-facing text, including the `tool.*.description` ids). Out of scope: tool names, tool JSON schemas, code, any automatic apply, history or rollback beyond the existing "Reset" of an override, notifications, a diff algorithm.

## Design

- `prompts::ProposalStore` (directory `<config root>/prompt-proposals`, a sibling of `prompts/`): one pending proposal per prompt id as `<id>.json` with `id`, `text`, `reason`, `base_hash`. A new proposal for the same id replaces the old one.
- `propose` validates like `set_override`: known id, non-empty text, valid placeholders; and also rejects text equal to the active text and an empty reason. `base_hash` is the active hash at proposal time. A listed proposal is `stale` when the active hash differs from `base_hash`.
- `approve` calls `set_override` then removes the proposal.
- Tool `prompt_propose` (`id`, `text`, `reason`) writes a proposal; `id` alone returns the active text, because the review's `read` cannot reach the config root. It is declared to every Session like `skill_manage`, which is how the review fork (a replay of the Session request) can call it. The review whitelist gains it.
- API: `GET /api/prompt-proposals`, `POST /api/prompt-proposals/{id}/approve`, `DELETE /api/prompt-proposals/{id}`.
- Web: a "Proposed changes" list in the Prompts settings with reason, stale flag, current and proposed text, Approve and Reject.
- `review.skills` tells the fork when to propose instead of writing a skill: guidance that applies to every instance of a tool or every Session belongs in a prompt.

## Acceptance

- [ ] `ProposalStore` tests: propose stores and lists; unknown id, empty text, bad placeholder, empty reason and unchanged text are refused; a second proposal replaces the first; `stale` turns true after the active text changes; approve writes the override and removes the proposal; discard removes it; a missing proposal is a not-found error.
- [ ] `prompt_propose` runs through the `Toolbox`, appears in the MCP tool list and in `OFFERED_TOOLS`, and has a `tool.prompt_propose.description` prompt.
- [ ] The review whitelist allows `prompt_propose`; non-whitelisted tools are still refused with the updated message.
- [ ] API tests: list, approve (override visible via `GET /api/prompts/{id}`), reject, 404 for an unknown proposal, 400 for an invalid text. `openapi.json` and the TypeScript client are regenerated.
- [ ] Web test: proposals render with reason and both texts, Approve and Reject call the API and refresh the list.
- [ ] `review.skills.md` and the owning `AGENTS.md` files (`crates/prompts`, `crates/tools`, `crates/api`, `crates/daemon`, `specs/phase-2/issues`) are current.
- [ ] A running Session keeps its pinned prompt text after an approval; a new Session gets the new text (existing pin tests cover the pin; one new test covers approval).
- [ ] `scripts/check` passes unchanged. Independent Spec and Standards reviewers report separately.

## Baseline and ownership

- Implementation starting point: `4057d7978fb6c5e0cd25c0f12ab483853f6f4fc3` plus a heavily dirty shared checkout. The exact tracked diff before this ticket is saved at `/root/.local/share/emberloop-rs/baselines/prompt-proposals.diff` and the untracked list at `/root/.local/share/emberloop-rs/baselines/prompt-proposals.untracked`. Everything in them is unrelated to this ticket except the earlier prompt edits listed next.
- Earlier same-day edits already in that baseline and not part of this ticket: `crates/prompts/prompts/tool.edit.md` (range and re-read rules), `crates/prompts/prompts/compress.prompt.md` (continuity rule), their assertions in `crates/prompts/src/tests.rs`, and the matching line in `crates/prompts/AGENTS.md`.
- Review the ticket's own changes with `git diff` against the baseline diff; do not flag unrelated dirty files.
- Do not commit without authorization.
