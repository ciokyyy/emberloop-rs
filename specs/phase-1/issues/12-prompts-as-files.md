# Ticket 12: Prompts as files

## Goal

Make every model-facing text editable without a rebuild: embedded defaults, an override file layer in `<config root>/prompts/`, `{{ name }}` placeholders validated at load, watcher reload with last-good fallback, per-Session pinning by content hash recorded in `meta.json`, `GET /api/prompts` and `PUT/DELETE /api/prompts/:id`, and a Prompts section in Settings. See spec decision I. Blocked by ticket 02; tickets 08 and 10 consume it.

## Status

Done except one gap: a broken override keeps the last good text and logs a warning, but no `error` event is emitted.

## Acceptance

- [x] Deleting an override restores the embedded default.
- [ ] A broken override keeps the last good text and emits an `error` event.
- [x] An unknown placeholder fails at load.
- [x] A running Session keeps its pinned text after a reload; a new Session gets the new text.
- [x] `meta.json` records one hash per prompt id the Session used.
- [x] No running Session ever loads a changed prompt, tool descriptions included, and there is no apply-now action.

## Tests first

Written and failing before any implementation, one per acceptance item: default used when no override exists; override wins; empty override falls back; unknown placeholder rejected at load; broken file keeps last good text and emits `error`; watcher reload seen by a new Session and not by a running one; `meta.json` hash set; the API lists, writes and deletes overrides.
