# Ticket 07: bash with views, background tool, restart persistence

## Goal

`bash` with seen-line feeds, `background` command management (list, read, wait, stop), process group isolation, daemon pid adoption on restart. Blocked by ticket 05.

## Status

Done. `bash.rs` and `background.rs`; process-group kill, merged output, views, offset read, wait, stop and pid plus boot-id adoption are tested.

## Tests first

Failing before implementation: `bash` kills the whole process group on timeout; output is merged and capped; `cat`, `head`, `tail`, `sed -n` and `grep -n` views feed the seen-line store; `background` list, read (offset), wait (`exited` or `timeout`) and stop; adoption after restart checks pid, start time and boot id; a reused pid is not adopted or killed; an exit while the daemon was down yields one notice exactly once with `exited (code unknown)`; a stop of an adopted group works.
