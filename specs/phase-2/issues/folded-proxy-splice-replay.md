# Fix folded proxy-tool exchanges replaying after compression

## Scope

Stop stored proxy-tool exchanges from being reinserted when an active compression block already covers them. Session `ses_ec3457d22904` completed 23 folds; by block b23, 27 previous compression calls reappeared with approximately 414,000 characters of arguments. A request rebounded from 21,886 to 160,384 input tokens.

The proxy currently applies compression before replaying splices. A folded exchange loses its marker and anchor, so splice replay restores it through its fallback path. Repeated folding therefore retains old compression arguments instead of removing them.

## Acceptance

- [x] A real proxy request that folds an earlier proxy-tool exchange does not replay that exchange on subsequent harness requests.
- [x] Nested folds, a runtime restart, and already-present summary markers retain that behavior.
- [x] Decompression restores the exchange exactly once; unrelated and newly created proxy exchanges still replay normally.
- [x] Existing stored blocks and splices work without destructive migration. Compression thresholds and provider handling do not change.
- [x] Focused regression tests fail before the fix and pass afterward; `scripts/check` passes unchanged.
- [x] Independent spec and standards reviewers report before release. Build a release and queue the running daemon update; verify the installed version after restart.

## Implementation baseline

- Starting point: `5b429e512922631ad099e162cd10345172631eba`.
- This is a shared, dirty checkout. Pre-existing work spans reviewer Sessions, model selection, tools, skills, and the web UI. `crates/proxy/src/server.rs` already has 20 added and 2 removed lines for reviewer behavior; `crates/proxy/AGENTS.md` already has 5 added lines. Preserve those changes. `crates/proxy/src/compressor.rs`, `crates/proxy/src/splice.rs`, and `crates/proxy/src/tests.rs` are clean at the starting point.
- Test seam: existing fake-upstream proxy HTTP tests, supplemented by native compressor/splice replay tests for persistence and decompression.
- Expected implementation scope: proxy replay/compressor code, their tests, and `crates/proxy/AGENTS.md`. No new dependencies or policy settings.
- Status: implemented, reviewed, deployed, and verified. The ticket remains open for the user; do not close automatically.

## Verification

- The HTTP regression failed before the fix with `folded exchange replayed` and the old `acp_status` call on the outgoing wire.
- `cargo test -p proxy --lib`: 225 passed. Coverage includes rebuilt runtimes, summaries already on the wire, nested folds, preserved splice bytes, and decompression without duplication.
- Updated the existing prefix-stability test that explicitly required replaying a pair after that pair was folded. It now requires one copy before folding and none afterward, while still checking prefix stability.
- `scripts/check`: exit 0, passed cleanly in 50s. The unchanged web tree reused its green build under the repository's normal check policy.
- Read-only coverage check against the affected Session's persisted state identifies 27 already-folded proxy exchanges to suppress and one current exchange to retain. No production records changed.

## Independent reviews

- Spec reviewer `ses_9fba548ed3f1`: pass; no missing requirements, scope creep, or incorrect implementation. Reviewer also ran `scripts/check` successfully.
- Standards reviewer `ses_8a24a030274d`: clean; no documented-standard violations or code smells in the scoped changes.
- Both reports arrived before the release build. Deployment and live verification completed afterward. The ticket remains open.

## Deployment

- Local release `0.0.71` built with `EMBERLOOP_HOME=/root/.local/share/emberloop-rs HOME=/root scripts/release`. Binary reports `emberloop-daemon 0.0.71`; artifact SHA-256: `c7cd0541f88d8883c49fcea5d275cea844df743d3d30e3ce522743eb4fff2461`.
- No commit or GitHub publication. The build includes the shared checkout's pre-existing changes.
- Orderly update completed from `0.0.70` to `0.0.71`. `current` points to `releases/0.0.71`; the installed binary and `/api/version` both report `0.0.71`. Serving process PID `3597828` resolves to `/root/.local/share/emberloop-rs/releases/0.0.71/emberloop-daemon`.
- Live verification Session: `ses_580915b29225` (`opencode-go/deepseek-v4.1-flash`). Before update, it folded the seed `acp_status` exchange into b1, call id `call_ec497942d69148fa9e99b1a1`. After update, the model folded eight older turns into b2. An assertion over b2's persisted originals confirms neither a call nor a result for that b1 id reappeared. The model returned `FOLD_REPLAY_VERIFIED` and preserved calibration word `cobalt`. Session finished idle without error events; no workspace edits. The test Session remains available.
