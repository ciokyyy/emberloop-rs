# Ticket 15: Model catalogue

## Goal

A `ModelCatalogue` service with a `Source` trait, an agy source discovered over ACP at `session/new`, merge by entry key, stale retention on a failing source, an atomic snapshot at `<data root>/models/catalogue.json`, a boot and 10-minute refresher, `GET /api/models` and `POST /api/models/refresh`. See spec decision K. Blocked by ticket 03.

## Status

Done. `crates/api/src/models.rs` with the `Source` seam, stale retention, atomic snapshot, boot and 10-minute refresh, and both endpoints; tested in `crates/api/tests/model_catalogue.rs`.

## Tests first

Failing before implementation: entries merge by key; a failing source keeps its last good entries marked `stale` with its error and time, and nothing empties; a restart serves the snapshot before the first refresh; the refresh endpoint refills the list; a fake second source proves the trait seam.
