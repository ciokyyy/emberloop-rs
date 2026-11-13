# Installable app: serving path and icons

Facts gathered from the code, so a PWA change starts from the real constraints.

## How the bundle is served

- `web/vite.config.ts` passes `@sveltejs/adapter-static` with `fallback: 'index.html'` to `sveltekit()` (SvelteKit 3 no longer reads `svelte.config.js`). `web/static/` holds the manifest, worker, icons and favicon; the adapter copies its contents into `web/build/`.
- `crates/daemon/src/lib.rs` embeds the bundle with `#[derive(RustEmbed)] #[folder = "../../web/build"] struct Assets`, and `router()` ends with `.fallback(static_asset)`.
- `static_asset` returns 404 for every `api/` path and serves `index.html` for every other unknown path, with the type from `content_type(path)`. An unmapped extension therefore gets `application/octet-stream`, and the shell fallback returns HTML under the requested name. Both break a manifest (Chrome refuses a `webmanifest` served as octet-stream or as HTML), so add `.webmanifest` to `content_type` and keep the file a real asset in `web/static/`. `js` is already mapped, so a service worker survives.
- `static_asset` sets no `Cache-Control`. Nothing is fingerprinted except `_app/immutable/*`, so a changed shell or service worker has only heuristic caching to rely on.

## Origin and tests

- `Config::default()` binds `127.0.0.1:3000`; `is_loopback` and `--allow-remote` gate a non-loopback host, and the API carries no token. Loopback is a secure context, so install and service workers work locally and nowhere else without TLS.
- The origin includes the port, so an installed app pins `:3000`; a daemon moved to another port is a different app.
- A host-relative `start_url` and `scope` of `/` keep the app usable behind the daemon's shell fallback for deep links; `/api/` is the one path that must never fall back.
- Loopback is not the only HTTPS origin in reach: read `/etc/caddy/Caddyfile` before proposing `tailscale serve` or `--allow-remote`. A hostname reverse-proxied to `127.0.0.1:<port>` gets a real certificate, so install, service workers and Web Push work on any device that resolves it, and a `remote_ip 100.64.0.0/10` block keeps it to the tailnet without opening the daemon. Two details decide whether the app survives that hop: SSE needs stream flushing (`flush_interval -1` in Caddy) or the feed stalls, and the installed origin becomes that hostname, not `localhost:<port>`. Compare the host the user typed against the Caddyfile before it hardens into a manifest, certificate and copy-link: a hostname recalled from memory is often one letter off.

## Icon geometry

- Artwork: `docs/logo.png`, 512x512 RGBA (the README image). Icons are always square PNGs; the mask that rounds them belongs to the platform.
- Ship 192 and 512 with `purpose: "any"`, one 512 with `purpose: "maskable"`, and a 180 `apple-touch-icon` link. Chromium no longer demands both 192 and 512, but the pair costs nothing.
- Maskable safe zone: the mark must fit the central circle of 80% of the width, because Android may crop to a circle or any other shape. iOS ignores `maskable` and applies its own squircle, and it uses `apple-touch-icon` when present, so the padding has to exist in the artwork.
- Safari on iOS supports `name`, `short_name`, `scope`, `display`, `start_url`, `icons` (15.4+), `theme_color` (15.0+); it ignores `background_color`, `orientation`, `shortcuts` and `dir`/`lang`. Since iOS 26 any added site opens as a web app, and `apple-mobile-web-app-capable` is no longer needed.
- `web/src/app.html` is where the `manifest` link, the `theme-color` meta and the icon links go; it currently carries only charset, viewport and the `emberloop` title.

## Notifications and sound

- The Notification API has no `sound` member, only `silent`. Custom audio comes from the page: play a short bundled file (`web/static/sounds/`, copied by adapter-static) or a Web Audio buffer, and pass `silent: true`, or the desktop notifier plays its own sound over it.
- Audio needs a user gesture first. Create or `resume()` the `AudioContext` in the first click through a one-shot global listener, not at notification time: after a reload the context starts `suspended` and `play()` is dropped without an error.
- Reach is the open page only. `EventSource` on `/api/sessions/{id}/events` already streams the feed (`web/src/lib/api/index.ts`, consumed in `web/src/routes/+layout.svelte`), so fire from the event handled there and throttle per session. A closed app gets no sound, because Web Push cannot choose one and iOS stops background audio.
- Ask for `Notification.requestPermission()` from a visible control (a Settings row with a sound preview), never on load: an on-load prompt is commonly auto-denied and that denial sticks.
- When the alert has to ring with no page open, the channel decides the sound and no web path can set one:

  | Channel | Custom sound | Self-hosted | Notes |
  |---|---|---|---|
  | Page audio, `silent: true` | your file | n/a | only while a window is open |
  | ntfy, Android app | per notification *channel*, chosen once in Android Settings | yes, no FCM | sound follows priority, not message; per-message sound is an open upstream request. A self-hosted server never uses FCM and its foreground service gives instant delivery |
  | Pushover | per message, `sound` | no | both platforms, paid, their servers |
  | Web Push, VAPID | none; OS default, and iOS plays none | no: FCM / Apple / Mozilla autopush | the daemon is the sender, so it needs outbound internet; see the Web Push section |
  | iOS, any web or self-hosted path | none | n/a | the sound file must ship inside a native app |
  | Tauri v2 mobile shell | yes | yes | the notification builder carries `sound` and `channel_id`; iOS still needs APNs to wake it |

- Desktop toasts from Rust are `notify-rust` (Linux, macOS, Windows), but a custom sound file is not portable across them because support depends on the desktop notifier, so play the file yourself and keep the toast silent. Ring on the machine the user looks at: a daemon on a remote host notifies that host, so the listener is a small client subscribing to the Session `EventSource` over the same tailnet.

## Web Push

- Push does not remove the icon or manifest work, it adds a service worker: a root-scope `sw.js` with a `push` listener, plus `notificationclick` that focuses an existing client or opens the Session URL. Ship no `fetch` handler, so nothing is cached and the shell cannot go stale.
- The daemon is the sender. Generate the VAPID keypair once into the data root, keep one subscription per browser (`POST /api/push/subscribe`, `/unsubscribe`) and use the `web-push` crate for the ES256 JWT and the RFC8291 payload encryption: hand-rolling either is a bug farm.
- Push costs reachability and privacy. The daemon needs outbound HTTPS to FCM, Apple or Mozilla autopush, and those services see the endpoint and timing of every alert although the payload is encrypted. Keep the body to the session title.
- iOS delivers only once the app is installed to the Home Screen, the permission prompt needs a user gesture, and the result carries no sound. Test one push on the real device before building a settings row around it.
- Dedupe in the daemon, never in the page. Have the page report its visible Session every ~20 s while focused and let the daemon skip the push when a client was visible within the last ~30 s. Any "is a tab open" test run in the client loses to a background tab, a sleeping laptop or a dead socket.
- Sound comes from the page only: `uisfx` (MIT, zero deps, session-verified at 0.4.0 — `createUISFX({pack:'minimal'}).play('success'|'error')`, `unlock()` from the first trusted gesture, ~12 kB gzipped runtime, no audio file fetched). `uifx` is the older cousin that plays files you supply. Check `https://registry.npmjs.org/<pkg>` (`dist-tags.latest`, `time.modified`, `dist.unpackedSize`) through `execute` before adding any UI dependency at all.
