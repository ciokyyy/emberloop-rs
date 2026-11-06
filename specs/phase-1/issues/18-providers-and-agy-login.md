# Ticket 18: Providers page and agy login

## Goal

Implement spec decision P: the sidebar Providers button and page, and the daemon-side agy login (spawn the pinned server for the Account's `GEMINI_HOME` with `--uid=`, ACP `authenticate` `oauth-personal`, sign-in URL from stderr, pasted loopback address, Account kept only after the server answers with its tier, cleanup on failure). Port the Go flow from `emberloop-web/agy_login.go` and `ProvidersPage.svelte` as reference. Blocked by tickets 03, 04 and 10.

## Status

Not done. No providers endpoints, login flow or Providers page. `AgyHarness` reads its account home from `<config root>/accounts/agy/default`, which nothing creates or signs in.

## Tests first

Failing before implementation, against an in-process fake agy server (no real Google login in the check): the sign-in URL printed on stderr reaches the panel; the pasted loopback address completes the login and the Account is kept with its tier; a failed, timed-out or cancelled login removes the Account and its home and kills the server; a second login while one runs answers 409; a missing server binary answers 503 with a clear message; the token file is never read, logged or returned by any endpoint; sign out deletes the Account and its home; a Session on a signed-out Account fails with a row linking to Providers; the Providers page shows signed in, signed out, in-progress and error states.
