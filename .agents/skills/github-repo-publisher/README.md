# GitHub Repo Publisher

[![Validate](https://github.com/MJWNA/github-repo-publisher/actions/workflows/validate.yml/badge.svg?branch=main)](https://github.com/MJWNA/github-repo-publisher/actions/workflows/validate.yml)
[![Release](https://img.shields.io/github/v/release/MJWNA/github-repo-publisher?style=flat)](https://github.com/MJWNA/github-repo-publisher/releases)
[![License](https://img.shields.io/github/license/MJWNA/github-repo-publisher?style=flat)](LICENSE)

GitHub Repo Publisher is a Codex skill for turning a local project into a complete, trustworthy GitHub repository. It guides an agent through repo metadata, public/private visibility decisions, README quality, community files, security defaults, branch protections, release setup, and final verification.

The goal is simple: when you ask an agent to publish a repo, it should not just push code. It should create a repository that explains itself, protects itself, and gives the next maintainer enough context to run, review, support, and improve it.

## What This Solves

Publishing a repository is easy to do badly. Common gaps include thin READMEs, missing security policy, no license, vague descriptions, accidental public visibility, incomplete topics, missing issue templates, weak branch protections, and no verification after creation.

This skill turns repo publishing into a repeatable workflow. It makes the agent define the repo contract, scan the local source, write repo-ready documentation, choose public or private safeguards, apply GitHub settings through `gh` and `gh api`, and report what was verified or skipped.

## Why It Matters

A repository is often the first operational interface for a project. A good one reduces onboarding time, avoids security mistakes, makes handoffs less fragile, and gives public users or internal teammates a clear path from "what is this?" to "I can run and maintain this."

For public repos, that means a clear value proposition, license, contribution path, security reporting route, topics, release notes, and enough technical detail for real adoption. For private repos, it means ownership, access boundaries, data classification, environment notes, safe configuration examples, and clear support routes without leaking sensitive details.

## Who It Is For

This skill is for Codex users and agent workflows that create, prepare, audit, update, or release GitHub repositories.

It is especially useful when:

- publishing a local project as a new public or private GitHub repo
- converting a rough project folder into a polished repository
- auditing an existing repo before sharing it
- upgrading repo settings, docs, and community health files
- preparing a repo for open-source release
- creating internal repos that need ownership and security hygiene

It is not a replacement for human judgement on licensing, legal review, sensitive data classification, or organization-level GitHub governance.

## How It Works

The skill gives Codex a structured publishing workflow:

1. Choose an operating mode and repo profile.
2. Capture the repo contract in `.repo-publisher.yml`.
3. Inspect the source tree, git state, commands, docs, and likely secret-bearing files.
4. Generate or improve README, community files, security files, templates, and optional workflows.
5. Create or update the GitHub repo with `gh repo create`, `gh repo edit`, and `gh api`.
6. Apply security and governance defaults when the repo and GitHub plan support them.
7. Verify the resulting metadata, topics, files, README completeness, rules, and settings.
8. Report the public/private URL, files changed, settings applied, skipped settings, and follow-up actions.

The workflow is intentionally conservative. It treats visibility as a security decision, avoids overwriting useful existing files, and records assumptions instead of inventing owners, commands, or deployment details.

## Features

- Public and private repo publishing guidance with different safeguards.
- Operating modes for new repos, existing repos, audits, updates, and releases.
- Profiles for minimal repos, internal tools, public open source, packages, docs sites, and strict governance.
- Profile-gated README badge guidance so repos get useful live status signals without badge clutter.
- Repo polish guidance for community profile, social preview, citation/funding, governance, maintenance state, and issue routing.
- README guidance focused on plain-English value, technical mechanics, commands, workflows, troubleshooting, and ownership.
- `.repo-publisher.yml` manifest template for repeatable repo decisions.
- Lightweight secret preflight script for local source scanning before publication.
- README completeness scoring script to catch thin docs and unresolved placeholders.
- GitHub CLI and API command references for repo metadata, topics, rulesets, Actions permissions, and security settings.
- Templates for security policy, support, contributing, code of conduct, PR template, Dependabot, rulesets, CodeQL, Scorecard, Release Drafter, and Probot Settings.
- Release workflow guidance for draft GitHub releases, Release Drafter, Release Please, and semantic-release.

## Repository Structure

```text
.
├── SKILL.md                         # Codex skill entrypoint and operating workflow
├── agents/
│   └── openai.yaml                  # Optional agent/workstream metadata
├── references/
│   ├── gh-command-reference.md      # Compact GitHub CLI and API command patterns
│   ├── profile-matrix.md            # Mode/profile selection and hardening defaults
│   ├── readme-badges.md             # Badge budgets, ordering, examples, and anti-patterns
│   ├── readme-patterns.md           # README requirements and public/private structures
│   ├── release-workflows.md         # Release automation decision guide
│   ├── repo-polish-conventions.md   # Community profile, social preview, governance, status
│   ├── repo-checklist.md            # End-to-end repo publishing checklist
│   └── security-settings.md         # Rulesets, protections, Actions, Dependabot, CODEOWNERS
├── scripts/
│   ├── readme-completeness.py       # README quality/checklist scorer
│   └── secret-preflight.sh          # Lightweight local secret and risky filename scan
└── templates/
    ├── README-public.md             # Public repo README template
    ├── README-private.md            # Private/internal repo README template
    ├── SECURITY.md                  # Security policy template
    ├── SUPPORT.md                   # Support policy template
    ├── CONTRIBUTING.md              # Contribution guide template
    ├── CODE_OF_CONDUCT.md           # Code of conduct template
    ├── .repo-publisher.yml          # Publishing manifest template
    ├── .github/                     # PR, CODEOWNERS, Dependabot, release templates
    ├── api/                         # GitHub ruleset API payload templates
    └── settings/                    # Settings-as-code examples
```

## Quick Start

Clone the repo and copy the skill into your Codex skills directory:

```bash
git clone https://github.com/MJWNA/github-repo-publisher.git
mkdir -p ~/.codex/skills/github-repo-publisher
rsync -a github-repo-publisher/ ~/.codex/skills/github-repo-publisher/
```

Ask Codex to use the skill:

```text
Use the github-repo-publisher skill to publish this local project as a public GitHub repo.
```

For private or internal work, be explicit:

```text
Use the github-repo-publisher skill to prepare this repo as a private internal tool. Include ownership, access, data classification, and security defaults.
```

Success looks like this: Codex states the publishing contract, scans the repo, writes or updates the repository files, creates or updates GitHub settings, verifies the result, and reports the repository URL plus any settings that could not be applied.

## Commands

| Command | Purpose |
|---|---|
| `bash scripts/secret-preflight.sh .` | Run a lightweight scan for risky filenames and obvious secret markers before publishing. |
| `python3 scripts/readme-completeness.py README.md` | Score whether a README covers the expected repo-ready sections. |
| `gh repo create OWNER/REPO --public --source=. --remote=origin --push` | Create and push a new public repo from a prepared local checkout. |
| `gh repo edit OWNER/REPO --description "..." --enable-issues --delete-branch-on-merge` | Apply common repository metadata and behavior settings. |
| `gh api repos/OWNER/REPO/topics --method PUT --field names[]="topic"` | Apply topics when high-level `gh` flags are not enough. |
| `gh api repos/OWNER/REPO/rulesets --input templates/api/ruleset-main-light.json` | Create a solo-friendly repository ruleset after checking branch names and plan support. |

## Configuration

The skill uses a manifest named `.repo-publisher.yml` to preserve publishing decisions:

```yaml
mode: new-repo
profile: public-oss
owner: MJWNA
repo_name: github-repo-publisher
visibility: public
description: "Codex skill for publishing complete, well-documented, secure GitHub repositories."
homepage: ""
source_path: "."
default_branch: main
license: mit
topics:
  - codex
  - github
  - repository-automation
  - readme
  - developer-tools
```

For each repo, update the manifest with the real owner, repo name, visibility, topics, license, maintainers, support route, and any profile-specific settings. Do not place tokens, credentials, secrets, private keys, or production URLs in the manifest.

## Usage

Typical prompt:

```text
Use github-repo-publisher on this project. Publish it as a public repo under MJWNA. Fill out the README properly, add community files, scan for secrets, apply topics, and verify the GitHub settings.
```

The skill should respond with a concise plan before remote actions unless you explicitly ask it to work autonomously. For a public repo, the resulting repository should normally include:

- `README.md`
- `LICENSE`
- `SECURITY.md`
- `SUPPORT.md`
- `CONTRIBUTING.md`
- `CODE_OF_CONDUCT.md`
- `.repo-publisher.yml`
- `.github/pull_request_template.md`
- `.github/ISSUE_TEMPLATE/*`
- `.github/CODEOWNERS` when owners are known
- `.github/dependabot.yml` when package ecosystems or workflows exist
- branch protection or rulesets when GitHub plan support allows

For private repos, the README should focus more on ownership, access, data boundaries, environments, deployment, runbooks, and operational support.

## Development Workflow

This repository is documentation and helper-script heavy. A normal change should:

1. Update `SKILL.md` only when the top-level workflow changes.
2. Put detailed guidance in `references/`.
3. Put reusable files in `templates/`.
4. Keep scripts small, dependency-light, and safe to run locally.
5. Run the validation checks before opening a pull request.

Suggested branch naming:

```text
feature/readme-quality-gate
fix/ruleset-template
docs/public-profile-guidance
```

## Testing And Verification

Run these checks locally:

```bash
bash -n scripts/secret-preflight.sh
python3 -m py_compile scripts/readme-completeness.py
python3 scripts/readme-completeness.py README.md
python3 - <<'PY'
from pathlib import Path
import json
for path in Path("templates/api").glob("*.json"):
    json.loads(path.read_text())
    print(f"ok {path}")
PY
```

When publishing a real repository, also verify with GitHub:

```bash
gh repo view OWNER/REPO --json nameWithOwner,visibility,description,homepageUrl,repositoryTopics,defaultBranchRef,isPrivate
gh api repos/OWNER/REPO --jq '{visibility,has_issues,has_wiki,has_discussions,delete_branch_on_merge,allow_squash_merge}'
gh api repos/OWNER/REPO/topics --jq '.names'
gh api repos/OWNER/REPO/rulesets --jq 'map({name,enforcement,target})'
```

## Release Process

This repo uses SemVer-style Git tags and GitHub Releases. It does not use heavy package-release automation because this is a Codex skill, not an npm/PyPI package.

While the skill is pre-1.0, minor versions may still change behavior. After `v1.0.0`:

- PATCH: wording fixes, documentation clarifications, non-breaking template fixes, and compatible script fixes.
- MINOR: new optional workflows, references, examples, templates, or compatible behavior.
- MAJOR: changed invocation contract, required tools, install layout, outputs, defaults, or breaking script behavior.

Recommended manual release flow:

```bash
git tag v0.1.0
git push origin v0.1.0
gh release create v0.1.0 --draft --generate-notes --title "v0.1.0"
```

Use Release Drafter or Release Please only if the repo starts receiving regular external contributions or versioned downstream usage.

## Troubleshooting

| Symptom | Likely Cause | Fix |
|---|---|---|
| Codex does not find the skill | The skill is not installed under `~/.codex/skills/github-repo-publisher`. | Copy or symlink the repository contents into that directory and restart the session if needed. |
| `gh repo create` fails | GitHub CLI is not authenticated or the repo already exists. | Run `gh auth status`, choose another repo name, or switch to update mode. |
| Ruleset creation fails | Rulesets may require a different plan, org policy, or adjusted payload. | Report the limitation and fall back to classic branch protection where appropriate. |
| README score is low | Required sections are missing or still contain placeholders. | Fill in value, commands, configuration, workflow, troubleshooting, support, security, and license sections. |
| Secret preflight reports findings | The scanner found risky filenames or marker patterns. | Review every finding before publishing. Use a dedicated scanner such as gitleaks or trufflehog for high-risk public releases. |

## Roadmap Or Status

Status: active early release.

Useful future improvements:

- Add optional gitleaks/trufflehog integration for stronger public-release scans.
- Add a small manifest validator for `.repo-publisher.yml`.
- Add more profile-specific README examples.
- Add safer API payload rendering that substitutes owner, repo, branch, and check names before applying settings.
- Add a custom social preview image for sharper link sharing.

## Support

Use GitHub issues for bugs, gaps, and improvement ideas. Use `SECURITY.md` for vulnerability reports or sensitive disclosure. Do not post secrets, tokens, customer data, private repo names, or production details in public issues.

## Contributing

Contributions are welcome. Start with `CONTRIBUTING.md`, keep changes focused, and explain how you tested the workflow or template change.

## Security

See `SECURITY.md`. This repo contains automation guidance for publishing repositories, so changes that touch visibility, branch protection, Actions permissions, secret scanning, or release automation should be reviewed carefully.

## License

MIT. See `LICENSE`.
