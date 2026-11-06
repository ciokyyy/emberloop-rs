# Ticket 09: Steer queue and delivery routes, background exit notices

## Goal

Per-session steer queue (`steer` or `notice`), proxy request injection (route 1) and idle wake (route 2), background exit notices feeding the steer queue. Blocked by tickets 07, 08.

## Status

Done. The queue, idle wake, proxy injection, byte-identical replay and background-exit notices are wired and tested in `crates/core`, `crates/proxy` and `crates/daemon/tests/runner.rs`.

## Tests first

Failing before implementation: an entry is delivered at the 2xx response head and requeued only on a non-2xx or a connection failure before it; a stream that dies after the head does not redeliver; a turn end races an in-flight request through one lock and wakes only when none is in flight and the queue is non-empty; a notice and a steer queued together cause one wake with one merged prompt; the queue survives a restart; the injected steer is replayed byte-identically on later requests; a splice for a turn the harness drops is dropped with it.
