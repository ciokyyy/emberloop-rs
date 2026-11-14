# Refresh every dependency and release

Use when the user asks to update all Cargo and web dependencies, not when bumping one crate.

## Cargo

1. Run `cargo update` first, then inspect the root `Cargo.toml` exact pins (`agent-client-protocol`, `rmcp`) and the `pi-ai` git rev in `crates/daemon/Cargo.toml`. Exact pins do not move on `cargo update`.
2. Bump majors deliberately, then fix code against the compiler. Use `cargo update --verbose` and `cargo tree -i <crate>` to see what holds a crate back; do not weaken a pin to silence a constraint.
3. Known cross-major edits:
   - reqwest 0.13 renamed feature `rustls-tls` to `rustls`; `query` and `form` are opt-in.
   - hmac 0.13 needs `KeyInit` in scope for `new_from_slice`.
   - sha2 0.11 digests no longer implement `LowerHex`; format with `hex::encode(Sha256::digest(...))`.
4. Run `cargo check --workspace --all-targets` before tests. Report crates still held back by dependents instead of forcing them.

## Web

1. Run `cd web && bun update --latest`, then `bun outdated` to list majors you chose to hold.
2. SvelteKit 3 reads configuration from `vite.config.ts`, not `svelte.config.js`: move `adapter` and `preprocess` into `sveltekit({...})`, delete the old file, keep the `$lib` alias, and point `tsconfig.json` at `$app/tsconfig`.
3. TypeScript 7's native compiler does not expose `ts.sys`, which SvelteKit's config loader needs; hold at the latest 6.x that satisfies SvelteKit's peer range until it does.
4. jsdom 31 normalizes `translate(0px, 0px)` to `translate(0px)`; assert with a regex, not an exact string.
5. When a config file disappears, update every source list that names it: `scripts/check` and `crates/daemon/tests/web_bundle.rs`. Otherwise the freshness test compares against a missing path.
6. Run `bun run check`, `bun run test`, then `bun run build`; `scripts/check` is the final gate. A dependency-only diff can be clean and still fail the embedded bundle test if `web/build` predates the sources.

## Release and proof

1. Stage `openapi.json` and `web/src/lib/api/schema.d.ts` before `scripts/check`; its generated stage compares them with the index.
2. Build with `scripts/release` and install through the daemon update command as in the ship workflow.
3. Prove the served bundle by matching a fingerprinted `_app/immutable` file's `md5sum` between `web/build` and `curl http://127.0.0.1:3000/...`; a dependency-only change has no new string to grep.
4. Load `http://127.0.0.1:3000/p/<project>/s/<session>` for three real Sessions and assert `document.querySelector('main').innerText.length > 0` after each.
