# Ticket 06: Hashline edit, seen-line store

## Goal

Hashline `edit` tool ported from Go grammar, supporting `PUT N.=M:`, `PUT <N:`, `PUT >N:`, `PUT >$:`, `CUT N`, `CUT N.=M`, snapshot tag checking and seen-line store. Blocked by ticket 05.

## Status

Done. `edit.rs` and `seen.rs`; ops, one-pass apply, stale and unseen refusals, full-hash check and persisted seen lines are tested.

## Tests first

Failing before implementation, ported from the Go hashline tests: each op (`PUT N.=M:`, `PUT <N:`, `PUT >N:`, `PUT >$:`, `CUT N`, `CUT N.=M`); one-pass application against the original; overlapping ops refused; a missing or placeholder tag refused; an edit anchored on unseen lines refused with the current lines; a stale tag refused with the current lines; the full sha256 check rejects a 16-bit tag collision; the seen-line store is per Session and survives a restart; the reply carries the new tag and the changed lines in context.
