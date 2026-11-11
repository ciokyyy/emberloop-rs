# Repo Publisher Profile Matrix

Choose one profile before creating or updating files. Profiles prevent overbuilding tiny repos and under-protecting important ones.

## Profiles

| Profile | Best for | Required | README badges | Optional / conditional |
|---|---|---|---|---|
| `minimal` | Scratch, demo, private experiment | README, `.gitignore`, visibility, description | None by default; CI only if it exists | License, CI, issue templates |
| `internal-tool` | Private business apps and operations tools | README-private, `.repo-publisher.yml`, SECURITY, CODEOWNERS, PR template, issue templates, CI, Dependabot, secret preflight | Usually none; private GitHub workflow badges only when useful to maintainers | rulesets, Actions read-only setting, CodeQL, runbook links |
| `public-oss` | Public/open-source repos | README-public, LICENSE, SECURITY, CONTRIBUTING, CODE_OF_CONDUCT, SUPPORT, issue/PR templates, topics, CI, Dependabot, rulesets | CI, license, release/version if released, optional coverage/security if real | Scorecard, CodeQL, release automation, social preview |
| `package` | npm/Python/Ruby/etc packages | public-oss baseline, package metadata, changelog/release flow, semver guidance | CI, package version, runtime support, license, coverage if enforced | Release Please, semantic-release, provenance/trusted publishing |
| `docs-site` | Documentation sites and examples | README, license decision, deploy notes, CI, link checking, docs publish workflow | Docs build/deploy, link check, license; avoid package badges | Pages/Vercel/Netlify settings, release notes |
| `strict` | Sensitive internal repos or mature public repos | strongest internal/public baseline, rulesets, CODEOWNERS, least privilege Actions, Dependabot, secret scanning, security read-back | Authoritative live badges only; no static health claims or vanity counters | CodeQL advanced setup, Scorecard, settings-as-code |

Read `readme-badges.md` before adding badges. A small badge row can build trust; too many badges become noise.

## New Repo Versus Existing Repo

New repo:

- Generate `.repo-publisher.yml` first.
- Generate files from templates with placeholders replaced.
- Create repo with explicit visibility.
- Push initial commit.
- Apply settings after the default branch exists.
- Verify via read-back.

Existing repo:

- Inspect current files and remotes first.
- Do not overwrite valuable README/community files blindly.
- Show a patch summary: create, update, leave unchanged, skipped.
- Prefer small additive improvements.
- Use `.repo-publisher.yml` if present to identify expected profile and prior decisions.
- Compare badges, releases, community profile, maintenance status, social preview, citation/funding, and governance against `repo-polish-conventions.md`.

## Drift / Update Mode

Use this when a repo was previously published but standards improved:

1. Read `.repo-publisher.yml`.
2. Compare current files/settings against the selected profile.
3. Report drift as `missing`, `stale`, `manual`, or `intentionally skipped`.
4. Patch only clear gaps.
5. Record skipped items with reasons in the final report.

## Research-Backed Source Patterns

- DSACMS repo-scaffolder: maturity tiers and existing-repo patch mode.
- projen: generated config as source of truth and drift detection.
- Copier/Cruft: committed answers and update/check/diff workflows.
- PyScaffold/Scientific Python Cookie: one command creates serious project hygiene.
- GitHub Safe Settings / Probot Settings: settings-as-code works best when protected by CODEOWNERS.
