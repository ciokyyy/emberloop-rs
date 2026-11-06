# Ticket 05: MCP server with read, grep, overflow store, jail

## Goal

MCP server in the daemon serving `read` and `grep`, with project root path jail and overflow spill store. Blocked by ticket 02.

## Status

Done. `crates/tools` serves `read`, `grep`, the jail and the overflow store over rmcp; cases in `crates/tools/tests/tools.rs`.

## Tests first

Failing before implementation: first confirm `rmcp` serves streamable HTTP with cancellation (else keep the hand-rolled server and test sessions and cancel); a path outside the Project root is refused, including through a symlink; `read` returns numbered lines and the `[path#TAG]` header; binary and over-2-MiB files are refused with a reason; output over the threshold spills and returns head, tail and a handle; `grep` honours `.gitignore`; every call emits `tool_started` and `tool_finished`.
