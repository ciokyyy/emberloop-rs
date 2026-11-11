---
name: github-repo-publisher
description: Use when creating, preparing, publishing, or polishing a GitHub repository so it has complete metadata, README documentation, community files, security defaults, branch rules, and public/private visibility safeguards.
---

# GitHub Repo Publisher

Use this skill to turn a local project or new idea into a complete GitHub repository. It complements `github-repo-discovery`: discovery finds repos; this skill creates or publishes them.

## Operating Modes

Pick the mode first and say it back before writing or publishing:

| Mode | Use when | Default posture |
|---|---|---|
| `new-repo` | Creating a repo from a local directory or idea | Generate manifest, files, repo, settings, then verify. |
| `existing-repo` | Polishing a repo that already exists | Audit first, show patch summary, preserve existing docs unless replacing is clearly better. |
| `audit` | Checking repo readiness without changing it | Report gaps against the selected profile. |
| `update` | Bringing an older repo up to the current publisher baseline | Use `.repo-publisher.yml` when present; show drift before patching. |
| `release` | Versioning, tagging, drafting, publishing, or verifying a release after repo setup | Align version files, changelog, manifest, tag, release notes, CI/artifacts, and latest-release read-back before calling it done. |

Use profiles rather than one-size-fits-all defaults: `minimal`, `internal-tool`, `public-oss`, `package`, `docs-site`, and `strict`. Read `references/profile-matrix.md` when choosing a profile. For public skill repositories, prefer lightweight SemVer tags plus reviewed GitHub Releases once the skill has users or a stable installable snapshot.

## Non-negotiables

- Treat visibility as a security decision, not a cosmetic setting.
- Do not publish secrets, customer data, exports, backups, `.env` files, private keys, credentials, or production dumps.
- For existing local projects, inspect the worktree before creating/pushing anything: `git status`, `git remote -v`, `git branch --show-current`, and likely secret-bearing files.
- For public repos, include a real license or explicitly state that no open-source license has been chosen.
- For private/internal repos, prefer least-privilege team access, CODEOWNERS, secret scanning, push protection, Dependabot, and read-only Actions token defaults where available.
- Treat `README.md` as the repo's operating explanation, not a placeholder: it must explain plain-English value, technical mechanics, workflow, commands, problems solved, and why the repo matters.
- Prefer non-overwrite updates for existing repos: show what will change, preserve useful existing content, and make the smallest patch that raises repo quality.
- When docs/settings may have changed, refresh with official GitHub docs, GitHub CLI help, or Context7 before applying risky settings.
- For versioned repos, do not publish or report completion until version metadata, changelog/release notes, tag, GitHub Release, CI/release workflow, and read-back checks agree.
- Use protected-branch-safe publishing by default: if `main` rejects direct pushes or rules require PRs, switch to branch -> PR -> green checks -> merge -> tag the merged default-branch commit.

## Workflow

1. Define the repo contract.
   - mode, profile, owner/org, repo name, description, homepage, visibility, default branch, source path, license, gitignore template, topics, maintainers, support/security contact, and whether it is public/open-source or private/internal.
   - versioning contract when releases are expected: SemVer/CalVer/manual/automated, version file, changelog style, release workflow, artifact expectations, current/latest tag, and installed/runtime copy paths if the repo publishes a skill or tool.
   - If visibility is not explicit and the project may contain proprietary or client/business data, default to private and say why.
   - Create or update `.repo-publisher.yml` so future sessions can audit/update the repo without re-deriving decisions.
2. Audit local source before publishing.
   - Check `gh auth status`, current git status/remotes/branch, default branch sync, latest local and remote tags/releases, and scan for obvious secret files.
   - For skill/tool repos with an installed runtime copy, compare source checkout, installed copy, and public repo state before editing; choose one source of truth and do not overwrite unrelated local changes.
   - Run `scripts/secret-preflight.sh <path>` from this skill when publishing local code.
   - Run `scripts/readme-completeness.py <path>/README.md` after generating or changing a README.
   - Do not push unrelated user changes or accidental local artifacts.
3. Generate the repository files.
   - Always: `README.md`, `.gitignore`, `SECURITY.md`, `.github/pull_request_template.md`, `.github/ISSUE_TEMPLATE/*`, `.github/CODEOWNERS` when owners are known, and CI if commands are known.
   - The README must be complete enough for a stakeholder to understand the value and for an engineer to run, verify, maintain, and extend the repo.
   - Add README badges only when they reflect live, useful signals for the selected profile. Read `references/readme-badges.md`; avoid badge soup, fake status badges, private-token badge URLs, and vanity counters without a purpose.
   - Public/community-facing: add `LICENSE`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SUPPORT.md`, release/changelog guidance, discoverability topics, repo polish conventions, and social preview notes.
   - Private/internal: add ownership, environment, deployment, data classification, access boundaries, and runbook links without exposing sensitive detail.
   - Reuse files under `templates/` when they fit; replace placeholders before committing.
4. Create or update the GitHub repo.
   - Prefer `gh repo create` for creation and `gh repo edit` for supported settings.
   - Use `gh api` for topics, rulesets, branch protection, Actions permissions, custom properties, and security settings not exposed by high-level `gh` flags.
   - Use git push or the contents API for files. Prefer a branch and PR when branch rules are unknown or active; only push directly to the default branch when the repo contract allows it.
5. Apply protections after the default branch exists.
   - Prefer rulesets when plan/org support allows; use classic branch protection as fallback.
   - For solo-maintainer or early public repos, prefer a light ruleset that blocks force pushes/deletion and allows PR merges without requiring a second reviewer.
   - Require review, code-owner review, last-push approval, or required status checks only when there are real reviewers/check names available; do not create a ruleset that deadlocks normal maintenance.
   - Use `templates/api/ruleset-main-light.json` or `templates/api/ruleset-main.json` for solo-friendly defaults, `templates/api/ruleset-main-strict.json` for mature team repos, and `templates/api/ruleset-tags-release.json` for versioned release tags.
6. Verify.
   - Re-read repo metadata with `gh repo view`.
   - Check topics, files, default branch, rules/protection, Actions permissions, and security features.
   - Check README completeness, unresolved placeholders, badge quality, badge targets, and private-token leakage.
   - For versioned releases, verify `VERSION` or package metadata, `CHANGELOG.md`, `.repo-publisher.yml`, local and remote tag, GitHub Release, latest release, release workflow/artifacts when present, and that the tag points at the intended default-branch commit.
   - For public repos, check the GitHub community profile and README rendering when possible.

## Visibility defaults

Use this gate before creating or changing visibility:

| Case | Default | Notes |
|---|---|---|
| Open-source library, public website starter, demo, portfolio, public docs | Public | Requires license, contribution expectations, security policy, polished README, topics, and leakage scan. |
| Client work, internal tool, proprietary business process, credential-adjacent code, database/admin tooling | Private | Requires explicit access plan, ownership, CODEOWNERS, and security settings. |
| Enterprise-wide reusable internal starter | Internal | Only if every enterprise member can safely read it. Otherwise private. |
| Unsure | Private | Visibility can be widened later after review. |

Before changing private to public, explicitly warn that code, Actions history/logs, activity, and forks/visibility side effects may become visible. Use the required `gh repo edit --accept-visibility-change-consequences` flag only when the user intentionally accepts that.

## Automation Surfaces

Use the narrowest reliable tool:

```bash
gh repo create OWNER/REPO --private --description "..." --homepage "..." --source=. --remote=origin --push
gh repo edit OWNER/REPO --description "..." --homepage "..." --enable-issues --delete-branch-on-merge
gh repo edit OWNER/REPO --add-topic "topic-one,topic-two"
gh api repos/OWNER/REPO --jq '{name,visibility,description,homepage,default_branch,has_issues,has_wiki,has_discussions}'
```

For settings beyond `gh repo edit`, use `gh api` against official GitHub endpoints. Common examples:

- Replace topics: `PUT /repos/{owner}/{repo}/topics`
- Create rulesets: `POST /repos/{owner}/{repo}/rulesets`
- Classic branch protection: `PUT /repos/{owner}/{repo}/branches/{branch}/protection`
- Actions permissions: repository Actions permissions endpoints
- Custom properties: repository/org custom properties endpoints
- Security analysis settings: repository update payload when supported by plan/licensing

Templates include API payloads under `templates/api/` as starting points for `gh api repos/OWNER/REPO/rulesets --input ...`. Edit branch names, bypass actors, and required status checks before applying.

## Release Mode

Use `release` mode only after repo readiness is good:

- Pick and state the version bump before editing: PATCH for docs/non-breaking fixes, MINOR for compatible new workflows/templates/behavior, MAJOR for changed invocation/install/output/default contracts.
- Update every version surface in the same PR: `VERSION`, package metadata, `CHANGELOG.md`, `.repo-publisher.yml` release fields, install/runtime manifests, and generated docs that display the current tag.
- Move shipped changelog bullets out of `Unreleased`; leave a fresh `Unreleased` placeholder for the next cycle.
- Use a clean branch from the current default branch, open a PR, wait for required checks, and merge before tagging when branch protection or rulesets apply.
- Create the tag on the merged default-branch commit, push it, and verify remote tag read-back before creating the GitHub Release.
- Draft release: use `gh release create TAG --draft --generate-notes` for simple repos. Publish only after notes/assets/checks are reviewed, unless the user explicitly asked for a fully automated release.
- Public Codex/agent skill repo: use SemVer-style tags plus reviewed GitHub Releases; keep automation optional until the repo is PR-heavy or package-like.
- If a release workflow exists, trigger or verify it, then confirm the workflow checked version/changelog/tag alignment and uploaded the intended artifacts.
- Read back `gh release view`, `gh release list`, `gh repo view --json latestRelease`, and `git ls-remote --tags` before calling the release complete.
- Release Drafter: good default when PR labels should accumulate human-reviewed release notes.
- Release Please: good when version/changelog/release should be prepared in a PR.
- semantic-release: only for repos that intentionally use Conventional Commits and automated package publishing.

Read `references/release-workflows.md` before adding release automation.

## What To Read Next

Load only the needed reference:

- `references/repo-checklist.md` for the complete public/private creation checklist, fields, and verification commands.
- `references/profile-matrix.md` for mode/profile selection and hardening defaults.
- `references/readme-patterns.md` for README structures and copy requirements.
- `references/readme-badges.md` for profile-gated README badges and status signal rules.
- `references/repo-polish-conventions.md` for community profile, social preview, citation/funding, governance, maintenance status, and repo trust surface conventions.
- `references/security-settings.md` for rulesets, branch protection, Actions, Dependabot, secret scanning, CODEOWNERS, and private repo safeguards.
- `references/release-workflows.md` for release automation selection.
- `references/gh-command-reference.md` for compact `gh` and `gh api` command patterns.
- `templates/` for reusable starter files and API payloads.
- `scripts/secret-preflight.sh` for a lightweight local leak check before publishing.
- `scripts/readme-completeness.py` for README completeness scoring.

## Output Standard

When invoked, return a concise publishing plan before destructive/remote actions unless the user explicitly asked to work autonomously. After completion, report:

- repo URL and visibility
- files created or updated
- settings applied
- protections/security features enabled or skipped with reasons
- PR, merge commit, version, tag, release URL, release workflow/artifact status, and installed/runtime parity when release mode was used
- verification commands/results
- any manual follow-up, especially social preview upload or plan/licensing limitations
