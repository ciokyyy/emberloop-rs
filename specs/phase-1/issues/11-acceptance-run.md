# Ticket 11: Phase 1 Acceptance Run

## Goal

Run the end-to-end acceptance checks:
1. Create Project and Session; send text + image; agent answers.
2. Agent reads, greps, edits (hashline), runs bash; each shows a tool row.
3. Stale tag edit is refused naming current lines.
4. Agent starts bash(background: true); command exits; notice reaches model via steer or idle prompt.
5. Steer sent mid-turn is injected at next request and re-spliced unchanged.
6. Long session agent calls compress, history shrinks, decompress restores turn.
7. Edit `<config root>/prompts/ponytail.md`: a new Session uses the new text, a running Session keeps its pinned text, and `meta.json` shows both hashes.
8. Restart the daemon: the Session resumes, a running background command is still readable, an exit during the downtime still produces one notice, and an `edit` still works because the seen-line store persisted.
9. A large-log Session opens by reading only its tail. A Session idle for 3 days is compressed by the sweep; opening it restores the same feed; a viewed-only restore loses its hot copy at the next sweep and keeps the archive; a resumed one replaces the archive and restarts the loop.
10. `scripts/check` passes with nothing skipped.

## Status

Not run live. `crates/daemon/tests/acceptance.rs` covers checks 1, 2, 7 and 8 against a fake upstream; checks 3 to 6 and 9 are covered by crate tests (`tools`, `proxy`, `daemon/tests/runner.rs`, `store/tests/archival.rs`) rather than one end-to-end run. Check 8 (the Session resumes after a daemon restart) was run live against the real `agy` on Argon: the Session answered a fact given before the restart. The rest of the live run has not happened.