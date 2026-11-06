# Ticket 13: Efficiency: segmented event log and budgets

## Goal

Replace the simple append-only log with segments plus a turn-offset index, coalesce events before writing, keep no history in daemon memory, serve a metadata-only Session list, log `timing` events for every start stage, and enforce the budgets as tests. See spec decision J. Blocked by ticket 02; do it before 03 and 04 build on the log.

## Status

Done. Segmented log, tail reads, coalescing, metadata-only Session listing and timing events are implemented and tested in `crates/store/tests/efficiency.rs` plus `event_log.rs` and `coalesce.rs`.

## Tests first

Failing before implementation: a tail read of a 1M-event log reads under a fixed byte count; tail time does not grow with log size; coalescing keeps lines within a set ratio of tokens streamed; a tool call stores only `started` and `finished`; a Session list request opens no event file; the idle daemon stays under its RSS budget with 1,000 Sessions on disk (budget set when measured); replay by `after=seq` crosses segment boundaries correctly.
