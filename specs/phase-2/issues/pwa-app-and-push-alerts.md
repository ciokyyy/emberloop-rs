# Installable app with push alerts

## Request and evidence

The web UI is only reachable as a browser tab at `https://agent.koboscintilla.com`, served by Caddy from the loopback daemon. The user asked for it to install as an app ("a PWA so I can have it as an app"), with a round-safe logo, no URL bar, a way to copy a Session link without one, FigCraft opening in its own window, and an alert when a turn finishes or fails — a sound while the app is open and a system notification while it is closed.

Constraints found in the tree and the host: `web/svelte.config.js` uses `adapter-static` with `fallback: 'index.html'`, there was no `web/static/` directory, and `crates/daemon/src/lib.rs` served every unknown path as `index.html` with `content_type` mapping no `webmanifest`. `docs/logo.png` is a 512x512 RGBA image whose mark fills 45% x 84% of the canvas on a `#0f1117` background with transparent rounded corners, so a mask would crop the mark and an unmodified copy would not install cleanly on either platform.

## Scope

Chromium and iOS install the app from the manifest: no URL bar because `display` is `standalone`, and the OS owns the rounding, so the work is a full-bleed background with the mark inside the maskable 80% safe circle. Copying a link moves to a long-press or right-click menu on the Session header, because an installed window has no URL bar. Alerts split by where the user is looking: the page plays its own sound and shows a notice bar while it is visible, and the daemon sends a Web Push (service worker `push` listener, VAPID, one subscription per browser) only when no page reported that Session within 30 s. The page heartbeats its visible Session every 20 s for that decision. Events are a finished turn (`status` with `state: idle`) and `error`.

Out of scope: caching or offline use (the worker has no `fetch` handler), custom sounds for a closed app (Web Push cannot choose one), a toast library, auth for the API, and any change to the Caddy or tailnet setup.

## Acceptance

- [x] `web/static/` ships `manifest.webmanifest` (`display: standalone`, `start_url`/`scope`/`id` `/`), a push-only `sw.js` and the icon set; `app.html` links the manifest, the theme colour, the favicon and the Apple touch icon.
- [x] `scripts/gen-icons` generates every icon from `docs/logo.png` in bun, fills the mark's own background full-bleed, fits the mark inside the maskable 80% safe circle, and `--check` fails on a stale committed icon; `scripts/gen-icons --check` passes in the check.
- [x] The daemon serves the manifest as `application/manifest+json` and the worker from the origin root, and `tests/web_bundle.rs` fails when the bundle misses the manifest, the worker or an icon.
- [x] The Session header opens a menu on a 450 ms hold or a right click with `Copy link` and `Copy session reference` (Session, Project with root, URL), covered by a component test.
- [x] FigCraft opens in its own window from the project drawer, and the embedded `/design` route still serves a deep link.
- [x] A finished turn or an error while the tab is visible plays one `uisfx` cue (`success`/`error`) and shows a dismissible notice bar; a replayed tail from before the page opened alerts nothing.
- [x] The daemon pushes `{title, body, url}` to every subscription when no page reported that Session within `GRACE` (30 s), drops a subscription the push service answers 404/410 for, and `POST /api/push/test` is the Settings row's gesture.
- [x] The VAPID keypair is generated once into the data root and survives a restart; unit tests cover the grace window, the keypair, subscribe/unsubscribe persistence and which events alert.
- [x] `scripts/check` passes.

## Baseline and ownership

- Implementation starting point: `55a25ee`.
- Shared dirty checkout: `.agents/skills/*` changes belong to the earlier skill refresh commit and stay untouched.
- `deny.toml` gains `MPL-2.0` (for `ece`) and two advisory ignores (RUSTSEC-2023-0071, RUSTSEC-2026-0387) reached only through `jwt-simple` > `superboring` for VAPID; both describe RSA PKCS#1 v1.5 decryption, which this tree never performs. The user should confirm that trade.
- Test seams: `api::push` unit tests over a temp data root, `SessionMenu.test.ts` and `notify.test.ts` under vitest, and `tests/web_bundle.rs` over the built bundle.

## Verification

`scripts/check` passes cleanly (fmt, deny, machete, clippy, openapi, web, nextest, doc, slowest) in 193 s, web stage 163 s inside its 180 s budget — the test pool runs `threads` because the forked pool re-paid the Svelte compile per worker file, which had put the stage at 224 s and the check at 254 s. The Fallow gate passes at `88 A` (floor 87.9); the `unit size -10.0` deduction is the pre-existing 671-line `+layout.svelte` and the generated `schema.d.ts`, and splitting the layout is follow-up work, not this ticket.

Release `0.0.87` is built, installed and serving: `readlink <data root>/current` is `releases/0.0.87`, `--version` and `/api/version` both report `0.0.87`, the daemon answers `/manifest.webmanifest` as `application/manifest+json`, and `/sw.js`, the icons and the favicon each with their own media type. The served `_app/immutable/nodes/0.CSmRzDzT.js` is byte-identical to `web/build` and carries `Copy session reference`; a live `127.0.0.1:3000` page registers `/sw.js` at scope `/`, and that worker is `activated` with `pushManager`. Install and push delivery still need the real device: one install, one test push from Settings with the app closed, and one turn finished with the app open.
