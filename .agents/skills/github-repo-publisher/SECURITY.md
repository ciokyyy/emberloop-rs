# Security Policy

## Supported Versions

This repository is an active early release. Security fixes apply to the current `main` branch unless a versioned release line is announced.

## Reporting A Vulnerability

Please do not report vulnerabilities through public issues if the report includes exploit details, secrets, private repository names, access tokens, or sensitive operational information.

Use GitHub's private vulnerability reporting feature if it is available for this repository. If it is not available, open a minimal public issue asking for a private contact route without including sensitive detail.

## What To Include

Useful reports include:

- affected file or workflow
- what the unsafe behavior allows
- how an agent or user could trigger it
- suggested safer behavior
- whether the issue affects public repos, private repos, or both

## Scope

In scope:

- guidance that could cause accidental public exposure
- unsafe GitHub Actions permissions
- misleading branch protection or ruleset templates
- secret handling mistakes
- documentation that encourages posting secrets publicly

Out of scope:

- generic GitHub account security questions
- vulnerabilities in third-party tools linked from this repo
- reports that require access to private systems not owned by the reporter

## Disclosure

Please allow reasonable time for review and remediation before public disclosure.
