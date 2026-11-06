# P2-1: pi-rust harness, pi model catalogue, multi-wire proxy

## Goal

Run Sessions on `pi --mode acp` (pi-rust, pinned v0.12.11) with the OpenCode Go provider, so the User can use emberloop-rs for real and catch errors. Spec: `spec.md` P2-1.

## Facts (probed 2026-11-05, pi v0.12.11 musl, fake key, `--offline`, `--no-builtin-tools`)

- pi has a built-in `opencode-go` provider (30 models; `opencode` Zen has 79 more). `OPENCODE_API_KEY` is enough; no network is needed to list models.
- `session/new` returns the catalogue in `models.availableModels` and in the `model` config option (ids `opencode-go/<model>`). `session/set_config_option {configId:"model"}` selects one. The catalogue `Source` keeps the `opencode-go/` entries.
- One provider `baseUrl` override catches every model, but pi picks the wire per model, so the proxy sees four request shapes (suffixes after the override):
  `/v1/messages` (Anthropic, `x-api-key`), `/responses` (OpenAI Responses), `/chat/completions` (OpenAI chat), `/models/<id>:streamGenerateContent?alt=sse` (Google).
- Official upstreams (docs.opencode.ai/docs/go; third-party guides say `opencode.ai/zen/go/v1`, which conflicts): `https://opencode.ai/inference/go/{openai/v1/chat/completions, openai/v1/responses, anthropic/v1/messages, google/v1beta/models/<id>}`. Not yet verified with a real key.
- HTTP MCP in `session/new` is refused by pi; MCP goes in the per-Session pi `mcp.json` with `exposure: direct` (spec P2-1).

## Design

- **Wire seam in `crates/proxy`, patterned on billion-context/acp-kernel** (TypeScript, MIT plus a credit-the-kernel term for products users see; port the design, credit it, and the User decides if that term applies to a Rust port): every wire has a codec pair `xToCore` / `coreToX` plus an SSE reader, and splice, steer, meter and compression work only on the core message model. Adding a wire adds one module under `crates/proxy/src/wire/`; nothing else changes. The current Cloud Code (agy) handling becomes the first codec behind the same seam, with its tests unchanged.
- **Routing:** the proxy detects the wire from the path suffix and forwards to that wire's configured upstream prefix. Unknown or unparseable bodies pass through untransformed.
- **Catalogue:** each entry carries `wire`. A model whose wire has no codec yet is listed `available: false` and cannot be selected, so no Session breaks mid-turn.
- **Key:** `<config root>/keys.json`, owner-only; the daemon passes it to pi explicitly and never lets pi inherit env keys.
- **Harness:** new `crates/harness-pi` implements the `Harness` trait: per-Session `HOME` with `models.json` (proxy `baseUrl`) and `mcp.json`, starts `pi --mode acp --no-builtin-tools --offline`, `forget` maps to `session/delete`. Binary: pinned release download with SHA256 check; path configurable.

## Order (each step test-first, lands only with `scripts/check` green)

1. `harness-pi` with an in-process fake ACP pi; the pi catalogue `Source` from a recorded `session/new` fixture (the probe above).
2. Wire seam; move the agy Cloud Code handling behind it, tests unchanged.
3. OpenAI chat codec, then Anthropic messages codec (about two thirds of the 30 models). Fixtures from billion-context tests where they exist.
4. One live `#[ignore]` test with the User's key: reach OpenCode Go through the proxy, settle the upstream URL conflict above.
5. Hook-up: workspace member, proxy dispatch, daemon registration, Providers entry for the key.
6. Later: Responses codec, then Google (the agy Google shape is nearly there).

## Open

- Steer, splice and compression on Responses and Google have no fixtures yet.
- Whether pi's `session/load` resume and long-Session memory hold up (spec P2-1 spike list).
