# README Badges And Status Signals

Badges should answer a real trust question quickly: does this repo build, is it released, what license applies, what runtime/package version is current, and is the project maintained? They should not become decoration.

## Sources

- GitHub workflow status badges: https://docs.github.com/actions/managing-workflow-runs/adding-a-workflow-status-badge
- Shields.io docs: https://shields.io/docs/
- Shields.io static badges: https://shields.io/docs/static-badges
- Codecov status badges: https://docs.codecov.com/docs/status-badges
- OpenSSF Scorecard badges: https://github.com/ossf/scorecard#scorecard-badges
- Read the Docs status badges: https://docs.readthedocs.com/platform/latest/badges.html

## Badge Budget

Default to 0-5 badges. More than five needs a clear reason and usually belongs only on package repos with real package, coverage, docs, and security signals.

| Profile | Default badge posture |
|---|---|
| `minimal` | No badges by default. Add CI only if CI exists and matters. |
| `internal-tool` | Prefer text status, owner, and runbook links. Private GitHub workflow badges are fine inside GitHub, but do not add external private-token badges. |
| `public-oss` | CI, license, release/version if released, optional coverage/security if those signals are real. |
| `package` | CI, package version, runtime support, license, coverage if enforced, downloads only when adoption matters. |
| `docs-site` | Docs deploy/build, link check, license. Avoid package badges unless the docs repo also publishes a package. |
| `strict` | Authoritative live signals only. No fake static health badges, no private-token badge URLs, no unowned services without rationale. |

## Recommended Order

1. CI/build status
2. Tests or coverage
3. Package or release version
4. Runtime/platform support
5. License
6. Security signal, only if real and maintained
7. Docs/deploy status for docs sites
8. Downloads/stars only for public packages where adoption matters

Put badges directly under the `# Title`, before the first paragraph. Keep style consistent, usually `flat` or `flat-square`.

## Examples

GitHub Actions workflow:

```md
[![Validate](https://github.com/OWNER/REPO/actions/workflows/validate.yml/badge.svg?branch=main)](https://github.com/OWNER/REPO/actions/workflows/validate.yml)
```

GitHub release:

```md
[![Release](https://img.shields.io/github/v/release/OWNER/REPO?style=flat)](https://github.com/OWNER/REPO/releases)
```

License:

```md
[![License](https://img.shields.io/github/license/OWNER/REPO?style=flat)](LICENSE)
```

npm package:

```md
[![npm](https://img.shields.io/npm/v/PACKAGE?style=flat)](https://www.npmjs.com/package/PACKAGE)
```

PyPI package:

```md
[![PyPI](https://img.shields.io/pypi/v/PACKAGE?style=flat)](https://pypi.org/project/PACKAGE/)
```

OpenSSF Scorecard, only if results are published:

```md
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/OWNER/REPO/badge)](https://securityscorecards.dev/viewer/?uri=github.com/OWNER/REPO)
```

## Do Not Add

- Static `passing`, `secure`, `production-ready`, or `100% coverage` badges that are not backed by a live system.
- Badge URLs with `token=`, `api_key=`, or other private credentials.
- Mixed badge styles that make the README look stitched together.
- Downloads/stars/forks/view counters for internal tools or early repos where they do not guide users.
- Badges for workflows that do not run on the default branch.
- Badges that link nowhere useful.

## Verification

Before publishing:

- Click every badge target or inspect the Markdown links.
- Confirm GitHub workflow badges reference real workflow filenames.
- Confirm private repos do not depend on external badge URLs that require tokens.
- Confirm security or coverage badges are backed by configured services.
- Run `scripts/readme-completeness.py README.md` and review badge warnings.
