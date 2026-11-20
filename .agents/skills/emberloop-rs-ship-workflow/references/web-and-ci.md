# Web bundle and CI

Pitfalls:
- A new required prop on a Svelte component breaks every existing test mount with a type error in `svelte-check`. Give new props a default (`onskills = () => {}`) unless every caller must pass one.
- A new required field on a schema-generated type (`UsageRecord`, `Session`, …) breaks every fixture that builds one by hand, and `svelte-check` reports only the first. Regenerate `openapi.json` and `web/src/lib/api/schema.d.ts` quickly while iterating with `cargo run -p api --bin dump_openapi && (cd web && bun run generate:api)`, then grep the type name across `web/src` (`grep -rln UsageRecord web/src`) and update every test helper (defaulted parameters keep existing cases unchanged).
- A new proxy event payload must keep the shape its web reader expects: the `web/src/lib/feed.ts` branch reads the fields the emitter names at the top level, so test the emitted event data, not only that the event fired.
- Svelte/Tailwind work in `web/`: load the `emberloop-rs-web-ui` skill for the token list, mobile layout habits and jsdom stubs.
- CI steps have an order the tests enforce: any step that regenerates a tracked web source (`bun run generate:api` writes `web/src/lib/api/schema.d.ts`) must run before the Web bundle step, and `bun install --frozen-lockfile` must run before it, or the step exits 127 (`openapi-typescript: command not found`) or `the_embedded_web_bundle_is_newer_than_its_sources` fails after a green build.
- The `bunx fallow health` score gate lives in the `emberloop-rs-web-ui` skill (`references/fallow-gate.md`): measure the baseline's penalty lines before a large refactor, and keep unconsumed helpers private.
