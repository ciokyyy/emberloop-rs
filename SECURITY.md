# Security Policy

emberloop-rs is a local daemon that runs coding agents with your provider
accounts, so a vulnerability here can reach your files and your credentials.

## Supported versions

The latest tag on `master` is the supported version. Older tags are snapshots;
fixes land on the tip.

## Reporting a vulnerability

Do not open a public issue. Use GitHub's private vulnerability reporting on this
repository (Security → Report a vulnerability). If that route is unavailable,
contact the maintainer through their GitHub profile (`@ciokyyy`) and say you have
a security report without posting details.

Include:

- affected commit or tag, and which binary or page
- impact: what an attacker gains
- reproduction steps, and a proof of concept if it is safe to share
- whether the issue needs a foreign origin, a malicious repository, a crafted
  MCP server, or a local account

You get an acknowledgement when the report is read, and updates until it is
resolved. Please allow a fix to ship before public disclosure.

## Scope

In scope:

- the loopback HTTP and SSE surface, including `Host` and `Origin` checks and
  the signed `/ctx/` gateway routes
- the proxy: wire translation, credential handling, and the files under the
  config root
- the MCP servers the daemon spawns and the tools they expose
- the embedded web bundle and its rendering of model output

Out of scope: a compromised local account with read access to the data root, a
provider breach, and anything that requires running untrusted code the user
explicitly asked to run.

## Reporting a leaked secret

If a credential reaches a commit, revoke it first, then report it privately. The
repository history cannot be recalled once pushed, so the fix is revocation plus
a fresh credential, not a history rewrite.

## What the project already does

- The daemon binds loopback only and rejects requests with a foreign `Host` or
  `Origin`.
- Provider keys and login files live under the config root, not in the
  repository.
- CI workflows pin action commits and use a read-only `GITHUB_TOKEN`.
