# Release Workflows

Only add release automation when the repo contract needs releases. Do not force package-release machinery onto internal apps.

## Decision Table

| Need | Default |
|---|---|
| Public Codex/agent skill repo | SemVer-style tags plus reviewed GitHub Releases; no heavy automation by default |
| Skill catalog or docs-only repo | No formal versioning unless users install pinned snapshots |
| Skill plus CLI/package/plugin | SemVer, changelog, and Release Please or semantic-release when publishing is automated |
| Time-sensitive reference skill | CalVer is acceptable if freshness is the contract, but document it clearly |
| Human-reviewed release notes from merged PRs | Release Drafter |
| Version/changelog/release prepared in a PR | Release Please |
| Fully automated package publishing from Conventional Commits | semantic-release |
| One-off initial release | `gh release create --draft --generate-notes` |
| Internal app with deployments but no public versions | Deployment notes in README, no release automation by default |

## Skill Repo Versioning

For a public Codex or agent skill, version the installable workflow when users may pin, compare, or roll back behavior. A skill's public contract is not just code: it includes trigger metadata, required tools, file layout, scripts, templates, references, output standard, and publishing behavior.

Use SemVer once the skill has a public contract:

- PATCH: wording fixes, docs clarifications, examples, non-breaking template fixes, and compatible script bug fixes.
- MINOR: new optional workflows, references, templates, profiles, or compatible behavior.
- MAJOR: changed invocation contract, required tools, install layout, output contract, script behavior, or defaults that could break existing users.

Keep pre-1.0 releases honest: `v0.x.y` means the skill is useful but the contract may still move.

## Versioned Release Checklist

Use this checklist whenever the repo contract says releases are required, the
user asks for a production version, or the work changed an installable
skill/tool:

1. Refresh state:
   - `gh auth status`
   - `git status -sb`
   - `git fetch --tags --prune`
   - `gh release list --limit 10`
   - `gh repo view OWNER/REPO --json defaultBranchRef,latestRelease,pushedAt`
2. Choose the bump:
   - PATCH for docs, examples, compatible bug fixes, and non-breaking template
     fixes.
   - MINOR for compatible new workflows, templates, references, profiles,
     commands, or behavior.
   - MAJOR for changed invocation contract, required tools, install layout,
     defaults, output contract, or migration burden.
3. Update version surfaces together:
   - `VERSION`, package metadata, lockfiles when relevant.
   - `CHANGELOG.md`, with shipped bullets moved out of `Unreleased`.
   - `.repo-publisher.yml` release fields such as `current_tag`,
     `version_file`, `changelog_file`, `release_workflow`, and artifact
     expectations.
   - README badges/examples/docs that display the current version.
   - Installed/runtime copies for skill repos after verification, not before.
4. Verify locally:
   - project tests/evaluators/builds.
   - README completeness if README changed.
   - secret preflight and, for public/high-risk repos, a dedicated history
     scanner if available.
   - release workflow scripts locally when the repo has them.
5. Publish through the repo's protection model:
   - direct default-branch push only when explicitly allowed.
   - otherwise branch -> PR -> required checks -> merge.
6. Tag the merged default-branch commit:
   - confirm `git rev-parse HEAD` is the intended release commit.
   - `git tag vX.Y.Z`
   - `git push origin vX.Y.Z`
   - verify `git ls-remote --tags origin vX.Y.Z`.
7. Create or update the GitHub Release:
   - include user-visible changes, verification, artifact notes, and a compare
     URL.
   - create a draft first when notes/assets require review or immutable
     releases/assets are in use.
   - publish only when the user or repo contract authorizes publishing.
8. Read back completion:
   - `gh release view vX.Y.Z --json tagName,name,isDraft,isPrerelease,publishedAt,url,targetCommitish`
   - `gh release list --limit 5`
   - `gh repo view OWNER/REPO --json latestRelease`
   - release workflow run/artifact status when present.
   - installed/runtime parity for skill/tool repos.

## Draft Release Flow

```bash
git tag v0.1.0
git push origin v0.1.0
gh release create v0.1.0 --draft --generate-notes --title "v0.1.0"
```

Before publishing:

- confirm tests/build passed
- confirm README and changelog are current
- confirm version metadata and `.repo-publisher.yml` release fields match the tag
- confirm the tag points at the intended default-branch commit
- inspect generated notes
- attach assets only if they are intentional deliverables

## Release Notes Quality

Borrowed from strong `gh-release` skills:

- find previous release/tag
- inspect commits since previous tag
- enrich with merged PR titles and labels when possible
- group notes by user-visible changes, fixes, docs, chores, and breaking changes
- include compare URL
- create draft first unless automation is explicitly approved
- avoid inline shell notes when Markdown contains backticks or substitutions; use a notes file or carefully quoted argument
- mention verification and any skipped checks with reasons

## Automation Templates

- `templates/.github/release-drafter.yml` for lightweight human-reviewed release notes.
- Consider Release Please for multi-language/package repos where version and changelog should be handled in PRs.
- Consider semantic-release only when Conventional Commits and package publishing are already part of the repo contract.

Sources:

- GitHub generated release notes: https://docs.github.com/repositories/releasing-projects-on-github/automatically-generated-release-notes
- Semantic Versioning: https://semver.org/
- CalVer: https://calver.org/
- Release Drafter: https://github.com/release-drafter/release-drafter
- Release Please: https://github.com/googleapis/release-please
- semantic-release: https://github.com/semantic-release/semantic-release
