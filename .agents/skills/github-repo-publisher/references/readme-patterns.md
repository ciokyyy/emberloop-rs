# README Patterns

GitHub recommends a README for every repository. For this skill, "complete README" means the README should sell and explain the repo properly in plain English, then back it up with concrete technical detail. It should be repo-ready, not marketing fluff.

Sources:
- GitHub README docs: https://docs.github.com/articles/about-readmes
- GitHub repository best practices: https://docs.github.com/en/enterprise-cloud@latest/repositories/creating-and-managing-repositories/best-practices-for-repositories

## README Standard

Every published repo README should answer:

- What is this?
- Who is it for?
- What problem does it solve?
- Why does it matter?
- How does it work technically?
- What is the normal workflow?
- What commands do I run?
- How is it configured?
- How do I test, build, deploy, or operate it?
- How do I troubleshoot it?
- Who owns it and where do I get help?

The first screen should be understandable to a non-specialist stakeholder. The rest should be useful to the next engineer who has to run, maintain, or extend it.

## Pre-Write Discovery

Before writing or rewriting a README, inspect the repo rather than guessing:

- top-level tree and package/config files
- scripts or tasks in `package.json`, `pyproject.toml`, `Makefile`, `justfile`, `Taskfile.yml`, etc.
- existing docs, examples, screenshots, API references, and changelogs
- CI/CD workflows and deployment targets
- database, queue, webhook, cron, or sync code
- environment variable names and safe placeholders
- tests and verification commands
- package metadata, release config, and license
- current issues or TODO markers only when they are clearly relevant

Do not invent commands, owners, deployment targets, or support routes. If a required README section cannot be completed from evidence, include a clear placeholder marker only when the user has asked for a draft; otherwise ask or record it as a follow-up before publishing.

## Public README Structure

Use this structure unless the project clearly needs something different:

~~~markdown
# Project Name

Plain-English one-paragraph summary. Say what the repo is, who it helps, and the main outcome it creates. Keep it aligned with the GitHub repo description.

## What This Solves

Explain the concrete problem, pain, or workflow gap this repo addresses. Name the before/after clearly.

## Why It Matters

Explain the practical value: time saved, risk reduced, quality improved, clearer workflow, cheaper operation, better developer experience, or better user/customer outcome.

## Who It Is For

List the intended users, maintainers, systems, or use cases. Also state who it is not for when that prevents misuse.

## How It Works

Describe the technical mechanics in plain English:

- main components
- data flow or request flow
- important integrations
- storage, APIs, queues, jobs, or background tasks
- key design choices and tradeoffs

Add a small diagram if it genuinely helps.

## Features

- Feature one with practical value
- Feature two with practical value
- Feature three with practical value

## Repository Structure

```text
.
├── path/        # what lives here
├── path/        # what lives here
└── file.ext     # why it matters
```

## Quick Start

```bash
# install dependencies
# configure environment
# run locally
```

State what success looks like after these commands run.

## Commands

| Command | Purpose |
|---|---|
| `command` | What it does |
| `command` | What it does |

## Configuration

Document required environment variable names, config files, defaults, and safe example values. Never include real secrets.

## Usage

Show the smallest realistic example and expected result. Include screenshots only when they clarify real behavior.

## Development Workflow

Explain the normal contributor workflow:

- branch naming or issue flow
- local setup
- tests
- lint/build
- PR expectations
- release process

## Testing

Explain the test strategy and exact commands. Include what is covered and what still needs manual verification.

## Deployment Or Release

Explain how this project is deployed, packaged, published, or released. Link to hosted docs or release notes when relevant.

## Troubleshooting

List common failure modes, likely causes, and fixes.

## Roadmap Or Status

State whether the project is experimental, active, stable, maintenance-only, deprecated, or archived.

## Support

Where to ask questions. Security issues go to `SECURITY.md`, not public issues.

## Contributing

Link to `CONTRIBUTING.md`, code of conduct, and issue templates.

## Security

Link to `SECURITY.md`.

## License

Link to `LICENSE`.
~~~

## Private/Internal README Structure

~~~markdown
# Project Name

Private/internal repository for BUSINESS_SYSTEM_OR_PROCESS. Explain in one paragraph what this system does, who relies on it, and what outcome it supports.

## What This Solves

Describe the operational problem or business workflow this repo handles. Be specific enough that a new teammate can understand why it exists.

## Why It Matters

Explain the business or operational impact: revenue, compliance, support load, reporting accuracy, manual work avoided, risk reduced, or customer/student/member outcome.

## Who Uses It

List internal teams, roles, systems, automations, or external services that depend on it.

## How It Works

Describe the technical mechanics:

- app/service boundaries
- data flow
- key integrations
- scheduled jobs/syncs/webhooks
- databases/storage
- auth and permissions model
- important tradeoffs or constraints

## Repository Structure

```text
.
├── path/        # what lives here
├── path/        # what lives here
└── file.ext     # why it matters
```

## Ownership

- Owner team:
- Technical owner:
- Business owner:
- Security/contact route:
- Escalation path:

## Access And Data

- Visibility:
- Data classification:
- Systems touched:
- Secrets location: reference only, never values.
- Customer/client/student data exposure:
- Production write paths:

## Local Development

```bash
# install dependencies
# configure local environment
# run locally
# run tests
```

State prerequisites, expected local URLs, seed data, and what a successful run looks like.

## Commands

| Command | Purpose |
|---|---|
| `command` | What it does |
| `command` | What it does |

## Configuration

List environment variable names, config files, external service setup, and safe placeholders. Never include real secrets.

## Workflow

Explain the normal operating workflow: how work enters the system, how it moves through the code, what outputs are produced, and who consumes them.

## Testing And Verification

List automated checks, manual checks, sample records, preview/staging validation, and known gaps.

## Environments

List development, staging, preview, and production environment names or safe links. Do not include credentials.

## Deployment

Explain how deploys happen, what triggers them, how rollback works, and where logs/runbooks live.

## Operational Notes

Document jobs, syncs, schedules, alerts, common failure modes, and support route.

## Troubleshooting

List common symptoms, likely causes, and exact checks/fixes.

## Maintenance Status

Active, maintenance, deprecated, or archived. Include replacement repo if applicable.
~~~

## Writing Rules

- Keep the repo description and README opening sentence aligned.
- Use plain English before deep technical detail.
- Explain value without hype. Avoid vague claims like "powerful", "seamless", or "revolutionary" unless backed by concrete behavior.
- Prefer concrete nouns and workflows over abstract positioning.
- Put long API references or screenshots in `/docs`; keep README complete enough to understand the repo without leaving the page.
- Include exact commands, expected outputs, and what success looks like.
- Use relative links for repo files.
- Do not include API keys, screenshots with secrets, production URLs that should not be broadly visible, or customer data.
- For public repos, make installation and first successful run obvious.
- For private repos, make ownership, data exposure, production write paths, and operational responsibility obvious.

## README QA Checklist

Run `scripts/readme-completeness.py README.md` after generating or changing a README. Use the result as a readiness gate, not as the only quality measure.

Manual checks:

- First paragraph matches the GitHub repo description.
- Non-specialist can understand value in the first screen.
- Engineer can run the repo from commands alone.
- Every command says what success looks like.
- Configuration names are documented with safe placeholders only.
- Troubleshooting has symptom, likely cause, and fix.
- Maintenance status is explicit: experimental, active, stable, maintenance-only, deprecated, or archived.
- Public repos link license, contributing, security, and support.
- Private repos include ownership, access/data boundaries, deployment notes, and escalation route.
- Links to repo files are relative.
- No `TODO`, `FIXME`, `lorem ipsum`, fake badges, example API keys, or placeholder maintainers remain in published output.

## Supporting Docs

- `SECURITY.md`: supported versions and private vulnerability reporting route.
- `CONTRIBUTING.md`: setup, branch flow, tests, PR expectations, issue workflow, maintainer response expectations.
- `SUPPORT.md`: how to get non-security help.
- `CODE_OF_CONDUCT.md`: only add one the maintainer is willing to enforce.
