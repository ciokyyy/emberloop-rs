# Changelog

## Unreleased

- Nothing yet.

## 0.2.0 - 2026-04-30

- Added an explicit release-mode front door that requires version metadata,
  changelog/release notes, tags, GitHub Releases, release workflow/artifact
  checks, and latest-release read-back to agree before completion.
- Added a versioned release checklist covering bump selection, version surface
  updates, PR-protected publishing, tag verification, GitHub Release creation,
  workflow/artifact verification, and installed runtime parity.
- Expanded the publish checklist, command reference, and manifest template with
  version files, changelog policy, release workflow fields, tag read-backs, and
  release verification commands.
- Added protected-branch-safe guidance so versioned releases use branch -> PR ->
  green checks -> merge -> tag when direct `main` pushes are blocked.

## 0.1.1 - 2026-04-27

- Relaxed the baked light/default main-branch rulesets so solo-maintainer repos can merge PRs without impossible self-review.
- Added profile-gated README badge guidance and badge quality checks.
- Added repo polish conventions covering community profile, social preview, citation/funding, governance, maintenance status, and issue routing.
- Added lightweight SemVer/GitHub Releases policy for public skill repositories.
- Added release-note configuration and optional repo-polish templates.

## 0.1.0 - 2026-04-27

- Initial public release of the GitHub Repo Publisher Codex skill.
- Added public/private repo publishing workflow.
- Added README quality guidance and completeness checker.
- Added repo publishing manifest template.
- Added GitHub security, ruleset, Dependabot, Scorecard, and release workflow templates.
