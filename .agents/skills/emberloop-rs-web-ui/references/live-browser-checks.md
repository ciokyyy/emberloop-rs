# Live browser checks that depend on a specific Session

A jsdom test cannot prove layout, virtualized-row mounting, or an action that runs on a
rendered answer. Use `agent-browser` against the running daemon.

## Check a source change before a build

Port 3000 serves `web/build/`, the last release build, so an unbuilt source edit shows
nothing there. Run the dev server against the live daemon instead (no build, no restart):

```
cd web && EMBERLOOP_DEV_API=http://127.0.0.1:3000 bun run dev --port 3001 --strictPort
```

Open `http://localhost:3001/p/<projectId>/s/<sessionId>`: `/api` is proxied, so the Session
data is the daemon's own. Vite binds whichever address the host `localhost` resolves to (on
this host `[::1]` only), so `http://127.0.0.1:3001` can refuse the connection while
`http://localhost:3001` answers, and the first request compiles the route (allow ~20 s
before calling the port dead). Stop the dev server when the check ends.

## Open the Session you mean

`/sessions/<id>` does not honour the id: it redirects to the last Session of the
project (`/p/<projectId>/s/<otherId>`). Open the project-scoped path instead:

```
agent-browser open 'http://127.0.0.1:3000/p/<projectId>/s/<sessionId>'
```

Get `projectId` from `curl -s localhost:3000/api/sessions`, keyed by the Session id.
Check `location.pathname` in the first eval; a wrong id loads a plausible feed and the
check silently tests another Session.

## The feed is virtualized

Only the rows in view exist in the DOM. Scroll the feed box to its end before counting:

```js
(() => { const f = document.querySelector('.h-full.overflow-y-auto.px-4.py-4');
  if (f) f.scrollTop = f.scrollHeight; return 'ok'; })()
```

An older answer you want to inspect may still not mount, and the window can hold one row
only. Which kinds mount changes with the Session and the scroll position, so read the same
metric from two or three Sessions at both `scrollTop = 0` and `scrollHeight` before
concluding a row kind or a container is missing.

## Force a known answer

To exercise one markdown feature deterministically, make a scratch Session in the
current project and tell the model exactly what to echo:

1. `POST /api/sessions` with `{"projectId": "<id>", "title": "...", "model": "<agy model>"}`.
2. `POST /api/sessions/<id>/prompt` with a text that pins the markdown, e.g.
   `Reply with exactly this markdown, nothing else: see [cfg](file:///root/projects/emberloop-rs/Cargo.toml) for details.`
   The prompt call can return an empty body; the turn still runs. Sleep ~10-15s.
3. Open the project-scoped Session URL, scroll the feed, then assert on the rendered DOM.
4. `DELETE /api/sessions/<id>` when done, and `agent-browser close`.

## Assert the rendered shape

For `file://` embeds (`fileEmbeds.ts`), the answer keeps its inline link and stacks a
card below it:

```js
(() => JSON.stringify({
  shelf: document.querySelectorAll('.md-files').length,
  files: document.querySelectorAll('.md-files > .md-file').length,
  links: document.querySelectorAll('a[data-file]').length }))()
```

Click `summary` in one eval, sleep, then read `open`, the number of `pre span[class]`
tokens and the line count. Zero token spans after opening means the highlighter did not
run, even though the disclosure opened.

## Measure a layout change

Widths are a DOM fact, not a class fact: read `getBoundingClientRect()` on both sides and
check it against the arithmetic the classes imply. On a 1280 viewport with the 280px
sidebar open, the feed scroller is 1000, and the `px-4` content box, every row, `.md`
assistant answer, `PlanCard`/working wrapper, and `Composer` card all match at 968.

```js
(() => { const r = (el) => el ? Math.round(el.getBoundingClientRect().width) : null;
  const s = document.querySelector('[data-testid=feed-scroller]');
  return JSON.stringify({ win: innerWidth,
    scroller: r(s), cont: r(s?.querySelector('div.w-full')),
    rows: Array.from(s?.querySelectorAll('[data-index]') || []).map(r),
    md: r(s?.querySelector('.md')),
    composer: r(document.querySelector('main > div.px-4.pb-4.pt-1 > div')),
    plan: r(document.querySelector('main > div.px-4 > div')),
    overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth }); })()
```

Run it from `agent-browser eval`, wrapped in the `(() => { ... })()` IIFE. Then
`agent-browser set viewport 390 844`, `open` the URL again (a viewport change alone does not
re-render the mobile branch) and repeat. Expect `md`, `composer`, `plan`, and rows to equal
the scroll container minus its padding, and `scrollWidth - clientWidth` of 0, on both. Measure the element the user names, not the
first match of a loose selector: `.bg-surface-3` also hits code blocks inside an answer, so
a bubble measurement needs the row wrapper (`[data-index] div.self-end`) and its right edge
against the scroller's inner edge.
