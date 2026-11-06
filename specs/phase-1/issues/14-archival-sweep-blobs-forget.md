# Ticket 14: Archival lifecycle, content-addressed blobs, harness forget

## Goal

Implement the active, archived, restored lifecycle of spec decision L: a 24-hour sweep that compresses Sessions idle for 3 days into an archive file (zstd, highest practical level); opening an archived Session decompresses it into a hot copy and keeps the archive; the next sweep, once the restore is 24 hours old, either returns a resumed Session to Active and deletes its stale archive, or deletes the hot copy of a merely viewed Session and keeps the archive. No Session is ever deleted by the sweep. Also content-addressed `blobs/<sha256>` and `forget(resume_token)` on the harness trait for User deletes. Confirm the zstd crate and its long-window mode exist and are maintained before depending on them. Blocked by tickets 13 and 03.

## Status

Done. `store/src/archive.rs` and `blobs.rs` implement the lifecycle, tested in `crates/store/tests/archival.rs`; the harness trait carries `forget`; the daemon runs the sweep at boot and every `archive_sweep_hours`.

## Tests first

Failing before implementation: the sweep skips a Session idle under 3 days, one with a live process, a running turn or a queued steer; a compressed log reads back identical events; the uncompressed log is removed only after the archive verifies; an interrupted compression leaves the original intact; opening an archived Session creates a hot copy, keeps the archive and sets `restoredAt`; a restore younger than 24 hours is left alone by the sweep; a viewed-only restore loses its hot copy at the sweep and keeps its archive with no recompression; a resumed restore returns to Active, its stale archive is deleted, and it is compressed again after 3 idle days; a second sweep over the same state changes nothing; the sweep never deletes a Session, its Project directory or its harness copy; a User delete calls `forget`; the same image in two Sessions is one blob and survives deleting one of them; png, jpeg and webp are not recompressed; every sweep logs what it compressed and removed.
