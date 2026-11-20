# Operator scripts and runbooks

Use when a ticket ships a command an operator runs (repair, adjustment, migration helper) or a runbook for a paid or irreversible procedure.

## Script

1. Red first. Run the documented invocation before the file exists and keep the failure. Add `--self-test` as the script's own gate: it must fail before the behaviour exists and pass with it.
2. Generate, do not execute. Print the SQL or commands, and with `--out` write the same text to a file. The operator runs it against the target. This keeps production credentials out of the script. Add `--help` usage that lists every documented form.
3. Make the self-test assert output and guards:
   - quoting (a value containing `'` is doubled);
   - missing-target guard (no parent row means no ledger row);
   - idempotency key and rerun no-op;
   - rejected zero or invalid amounts, unknown flags and missing values.
   Put validation in the generator function, not only in argument parsing, so every entry point shares it. A self-test that checks only `parseArgs` misses a generator that prints an unguarded statement.
4. Match the production invariant. Copy the guard shape from the service that owns the table, including update-before-insert and the unique key; a repair key derives from the resource id. State when the runner's transaction semantics are unverified.
5. Completion: `--self-test` exits 0; every documented form generates once; a rerun of the same output is a no-op by inspection.

## Runbook

1. Verify every command, flag, env name and state claim against code and config at write time. A value that looks secret may be a variable; a migration may be pending, not applied. Never write "already applied" without checking.
2. Separate the stores: secret manager for secrets, config vars for non-secret ids and modes. Name the warning for a secret that signs users out or makes existing ciphertext unreadable.
3. Isolate test-mode and destructive steps: name the test environment, warn before SQL that can reach production, and give the exact target. For every guarded write, give a read-back query and the row it must show; a guard that matches nothing still exits successfully.
4. State the boundary of live state: when the target lives outside the database (a Durable Object, queue or vendor), give the UI or API path that acts on it and say when no operator CLI exists. Print no SQL that cannot see that state; give the escalation instead.
5. Write exact lists and values: webhook events, endpoint URL, presets, coupon terms, price. Point at the module that validates each one.
6. Rollback: say whether a down migration exists; give the exact reverse SQL; separate additive schema from data-destroying changes; require a backup for the latter; tell the operator not to edit migration bookkeeping.
7. Completion: every step the ticket names appears; every command was checked; the only placeholders left are values only the operator can supply.
