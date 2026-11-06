# AGENTS.md — crates/store

## Purpose

Filesystem persistence and storage layer:
- `DataRoot`: manages layout `<data_root>/projects.json`, `<data_root>/blobs/<sha256>`, `<data_root>/models/catalogue.json`, and `<data_root>/sessions/<id>/{meta.json (carries `resumeToken`), events/NNNNNN.jsonl, events/index.json, events.zstd, splices.jsonl, images/, spill/}`
- `ProjectsStore`: atomic JSON store for Projects (`{id, name, root, ...}`)
- `SessionsStore`: atomic metadata store for Sessions (`{id, project_id, title, ponytail_level, model, promptHashes, lastUsedAt, archivedAt, restoredAt, state, ...}`)
- `EventLog`: segmented append-only log with a sidecar segment/turn index, bounded live ring and tail reads
- `BlobStore`: content-addressed blobs, one file per sha256
- `EventCoalescer` and `CoalescedWriter`: assistant-text buffer and one-event writes
- `ArchivalSweeper`: active -> archived -> restored lifecycle

## Event log

Segments rotate at 4 MiB. `events/index.json` records each segment and each Prompt
turn's segment plus byte offset; append rewrites it only on rotation or a turn
start. `open` reads the sidecar and the last segment, never older events. `tail`
reads newest segments until the window of N blocks is filled (`blocks.rs`: a chunk run, a whole tool call, or a prompt/notice/error is one block; a window never cuts a tool from its start). The in-memory ring keeps the
last 200 events; replay of anything older reads files. `SessionsStore::load` and
`list` read only `meta.json`.

## Archival

`ArchivalConfig` defaults to a 24-hour sweep, 3 idle days before archiving and a
24-hour restore grace. `sweep_at` compresses `.jsonl` segments and `index.json`
into `events.zstd` (zstd level 19, long-distance matching), verifies the decompressed
bytes, renames the temp archive and only then removes the hot directory. A
restore decompresses into `events/`, keeps the archive and records `restoredAt`
and `restoredSeq`. The next sweep after the grace either deletes the stale
archive for a resumed Session or removes the hot copy for a viewed one.
