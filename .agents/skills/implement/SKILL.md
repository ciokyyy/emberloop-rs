---
name: implement
description: "Implement a piece of work based on a spec or set of tickets."
disable-model-invocation: true
---

Implement the work described by the user in the spec or tickets.

If the user passes a ticket reference, fetch it from the issue tracker and state its title before starting. If the reference is ambiguous, ask.

Before editing each ticket, record its local ticket/specification path and resolve `HEAD` to an immutable implementation starting point. Record any pre-existing dirty work so reviewers can distinguish it from the ticket's changes. If the ticket exists only in the tracker, fetch it into a local ticket file first.

Call the Skill tool with "tdd" where possible, at pre-agreed seams.

Run typechecking regularly, single test files regularly, and the full test suite once at the end.

After implementing and checking each ticket, pause edits and invoke `code-review` with that ticket path and captured starting point. Dispatch both reviewers, wait for both submitted reports, and present them separately before continuing to the next ticket. Reuse the same reviewer pair across the assigned specification. Do not close tickets or start an automatic fix/review loop.

Commit only when the user authorizes it and the repository's required checks pass.
