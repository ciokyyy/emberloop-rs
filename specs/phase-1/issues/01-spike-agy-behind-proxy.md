# Ticket 01: Spike: agy behind the proxy

## Goal

Validate that `agy` works behind an in-process / local proxy speaking Cloud Code API wire: inject one tool, run it, store the turn, re-splice byte-exact on request 2, verify thought signatures survive, and pin compression trigger rules.

## Status

Completed. PASS.

## Findings

1. **Server spawn**:
   - Pinned `agy_acp_server.par` requires `--uid=` argument on spawn to prevent group switch crash (`LookupGIDByGroupName nobody`).
   - Env variables `GEMINI_HOME` and `AGY_ACP_CCPA_BASE_URL` configure account credentials and upstream proxy target.

2. **ACP Protocol**:
   - `initialize`, `session/new`, `session/set_config_option` (model: `gemini-3.8-flash`), `session/prompt` all work over stdio JSON-RPC.

3. **Proxy Tool Injection & Swallowing**:
   - Proxy injected `spike_probe_tool` into `request.tools` on Cloud Code `v1internal:streamGenerateContent` wire.
   - Upstream Gemini model generated `functionCall` part carrying valid `thoughtSignature`.
   - Proxy intercepted and swallowed the tool call from agy, executed it locally, and forwarded the synthesized turn upstream. Upstream returned HTTP 200 with text completion.

4. **Thought Signature Survival across Re-splice**:
   - Proxy stored model turn part with `thoughtSignature` and user `functionResponse` part.
   - On subsequent ACP prompt turn (where agy's history omitted the swallowed tool call), proxy re-spliced the stored turn byte-exact into `contents`.
   - Upstream Cloud Code accepted re-spliced contents with HTTP 200 (no HTTP 400), and model answered `SPIKE_VERIFIED_PASS`.
   - Thought signatures survive byte-exact re-splice.

5. **Compression Trigger Rules**:
   - Pure model-driven billion context tools (`compress`, `decompress`, `search_context`).
   - No threshold setting and no automatic compression. Proxy injects tool schemas and system prompt rules.
