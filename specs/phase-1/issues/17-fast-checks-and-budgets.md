# Ticket 17: Fast checks and budgets

## Goal

Implement spec decision O: nextest config with a 4 s per-test terminate cap, clippy `disallowed-methods` for real sleeps, per-stage timing in `scripts/check` with budgets in `scripts/check-budget.tsv` (loop check 30 s or less, full check 3 minutes or less on this host), parallel stages, `scripts/check-changed` using reverse dependencies from `cargo metadata`, the mold linker and light dev profiles, and a slow-test report. Blocked by ticket 02. Phase 2 starts only when the measured phase 1 check meets the budgets.

## Status

Not done. `scripts/` holds only `check`: no `check-changed`, `check-budget.tsv`, nextest config with the 4 s cap, `disallowed-methods`, mold profile or slow-test report. The full check currently runs past the 3 minute budget.

## Tests first

Failing before implementation: `scripts/check` fails when a stage exceeds its budget; a test that sleeps past 4 s is terminated by nextest; `check-changed` selects a changed crate and its reverse dependencies and hands off to the full check on a manifest change; the check refuses a test using `std::thread::sleep` or `tokio::time::sleep` outside the fake clock module; the slow-test report lists the top 10.
