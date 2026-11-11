# GitHub Repo Publishing Checklist

Use this checklist when creating or polishing a repository.

## Source-backed Baseline

- GitHub repo creation supports owner, name, description, visibility, README, gitignore, license, template, and starter files through the web UI, GitHub CLI, and REST API.
- `gh repo create` supports `--public`, `--private`, `--internal`, `--description`, `--homepage`, `--add-readme`, `--gitignore`, `--license`, `--template`, `--team`, `--source`, `--push`, and `--clone`.
- REST create/update supports repo feature flags and merge settings such as issues, projects, wiki, discussions, squash/merge/rebase options, auto-merge, delete-branch-on-merge, template status, custom properties, and security analysis fields where available.
- Topics are public names even for private repos; use lowercase letters, numbers, and hyphens, max 50 chars each, max 20 topics.
- Social preview upload is mostly browser/manual. Recommended image: PNG/JPG/GIF under 1 MB, at least 640x320, ideally 1280x640.

Sources:
- GitHub creating a new repository: https://docs.github.com/en/repositories/creating-and-managing-repositories/creating-a-new-repository
- GitHub REST repositories: https://docs.github.com/en/rest/repos/repos?apiVersion=2022-11-28
- GitHub CLI `gh repo create`: https://cli.github.com/manual/gh_repo_create
- GitHub topics: https://docs.github.com/en/github/administering-a-repository/classifying-your-repository-with-topics
- GitHub social preview: https://docs.github.com/en/github/administering-a-repository/customizing-your-repositorys-social-media-preview

## P0 Identity And Creation

Capture before creating:

```yaml
mode: new-repo # new-repo | existing-repo | audit | update | release
profile: internal-tool # minimal | internal-tool | public-oss | package | docs-site | strict
owner: ""
repo_name: ""
visibility: private # public | private | internal
description: ""
homepage: ""
source_path: "."
default_branch: main
license: "" # public repos should choose one, e.g. mit, apache-2.0
gitignore_template: ""
topics: []
team: ""
template_repo: ""
```

Write these answers to `.repo-publisher.yml` so future sessions can audit or update the repo consistently.

Creation patterns:

```bash
gh repo create OWNER/REPO --private --description "..." --homepage "..." --source=. --remote=origin --push
gh repo create OWNER/REPO --public --description "..." --homepage "..." --add-readme --gitignore Node --license mit
gh repo create OWNER/REPO --internal --team "team-name"
```

For existing local repos, avoid `--add-readme`, `--gitignore`, and `--license` if equivalent local files already exist or would create conflicts.

## P1 Files

Always consider:

- `.repo-publisher.yml`
- `README.md`
- `.gitignore`
- `SECURITY.md`
- `.github/ISSUE_TEMPLATE/bug_report.yml`
- `.github/ISSUE_TEMPLATE/feature_request.yml`
- `.github/ISSUE_TEMPLATE/config.yml`
- `.github/pull_request_template.md`
- `.github/CODEOWNERS` if owners are known
- `.github/workflows/ci.yml` if build/test commands are known
- `.github/dependabot.yml` if package ecosystems are known
- `VERSION` or package version metadata when the repo is versioned
- `CHANGELOG.md` or an explicit generated-release-notes policy when releases are expected
- release workflow/configuration when release artifacts or gates are expected

Public/community-facing:

- `LICENSE`
- `CONTRIBUTING.md`
- `CODE_OF_CONDUCT.md`
- `SUPPORT.md`
- `CHANGELOG.md` or release notes process

Private/internal:

- ownership section in README
- environment/deployment notes without secrets
- data classification
- access model
- on-call/support route
- links to private runbooks

## P1 Repo Settings

Apply when appropriate:

- description and homepage
- topics
- issues enabled if the repo accepts work/support
- discussions enabled only for community Q&A or broad collaboration
- wiki disabled unless intentionally used
- projects enabled only if repo-level project tracking is used
- delete branch on merge enabled
- squash merge enabled for clean history; disable unused merge methods if the team has a convention
- default branch set to `main` after it exists
- repository custom properties for org governance, e.g. owner team, service, environment, data classification

## P1 Versioning And Release Readiness

For versioned repos, especially installable skills, CLIs, packages, plugins, and
templates:

- Choose SemVer unless the repo contract explicitly prefers CalVer or another
  scheme.
- Capture release strategy in `.repo-publisher.yml`: current tag, version file,
  changelog file, release workflow, artifact expectations, and whether releases
  are draft-reviewed or automated.
- Keep `Unreleased` for future work and move shipped bullets into the new
  release section.
- Verify the new tag does not already exist locally or remotely before creating
  it.
- Tag only the merged default-branch commit that passed checks.
- Confirm `VERSION` or package metadata, changelog, manifest, tag, GitHub
  Release, and latest release all agree.
- Run any manual release workflow and verify artifact upload when the repo has
  one.
- For installed skills/tools, sync the runtime copy only after the repo release
  checks pass, then verify installed/runtime parity.

## P1 Protection And Security

See `security-settings.md` for details. Minimum target:

- ruleset or branch protection on default branch
- require pull requests
- for solo-maintainer/early repos, allow PR merge with zero required approving reviews to avoid self-review deadlock
- require at least one approval only when another reviewer exists
- require code-owner review only when CODEOWNERS maps to real approvers
- require status checks after CI exists and check names are known
- block force pushes and deletion
- read-only default Actions token
- Dependabot alerts/security updates/version updates where useful
- secret scanning and push protection where plan allows

## P2 Public Polish

For public repos:

- README has install, quickstart, usage, docs, support, security, contributing, license, and status.
- License is detectable at repo root.
- Topics cover language, framework, domain, product category, and audience.
- Release process exists: `gh release create v0.1.0 --generate-notes --draft` when a first version is useful.
- Community profile should show README, license, contributing, code of conduct, issue templates, PR template, and security policy.
- Add a social preview image manually if the repo will be shared publicly.

## P2 Private/Internal Polish

For private/internal repos:

- README says who owns it, what business system it supports, how to run it, where it deploys, and how to get help.
- Access is via teams, not ad hoc individuals, except documented exceptions.
- Forking is disabled unless needed.
- CODEOWNERS covers `.github/workflows/`, infra, auth, billing, database, deployment, and security-sensitive paths.
- Audit log relevant changes if in an org: repo visibility, rulesets, secret scanning, Dependabot, Actions settings, invitations, access changes.
- Archive stale repos only after README/description name replacement, support status, and open issue/PR cleanup.

## Verification

```bash
python /path/to/github-repo-publisher/scripts/readme-completeness.py README.md
gh repo view OWNER/REPO --json nameWithOwner,visibility,description,homepageUrl,repositoryTopics,defaultBranchRef,isPrivate
gh api repos/OWNER/REPO --jq '{visibility,has_issues,has_wiki,has_discussions,delete_branch_on_merge,allow_squash_merge,allow_merge_commit,allow_rebase_merge}'
gh api repos/OWNER/REPO/topics --jq '.names'
gh api repos/OWNER/REPO/rulesets --jq 'map({name,enforcement,target})'
gh api repos/OWNER/REPO/contents/README.md --jq '.html_url'
gh release list --limit 10
gh release view vX.Y.Z --json tagName,name,isDraft,isPrerelease,publishedAt,url,targetCommitish
gh repo view OWNER/REPO --json latestRelease
git ls-remote --tags origin vX.Y.Z
```

If verification fails because of plan/licensing, report exactly what was skipped and why.
