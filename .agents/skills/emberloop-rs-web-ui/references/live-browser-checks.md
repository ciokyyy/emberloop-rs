# Live browser checks that depend on a specific Session

A jsdom test cannot prove layout, virtualized-row mounting, or an action that runs on a
rendered answer. Use `agent-browser` against the running daemon.

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

An older answer you want to inspect may still not mount. Do not conclude the feature is
missing from a zero count.

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
