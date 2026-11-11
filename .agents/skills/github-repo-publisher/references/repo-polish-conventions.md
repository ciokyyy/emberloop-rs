# Repo Polish Conventions

Repo polish is not just nicer files. It is the public trust surface of the repository: identity, support routes, maintenance state, release history, governance, security posture, and visual proof that the project is alive and usable.

## Sources

- GitHub community profiles: https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions/about-community-profiles-for-public-repositories
- Default community health files: https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions/creating-a-default-community-health-file
- Support resources: https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions/adding-support-resources-to-your-project
- Issue templates and forms: https://docs.github.com/en/communities/using-templates-to-encourage-useful-issues-and-pull-requests/configuring-issue-templates-for-your-repository
- CODEOWNERS: https://docs.github.com/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners
- Repository topics: https://docs.github.com/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/classifying-your-repository-with-topics
- Social preview: https://docs.github.com/en/github/administering-a-repository/customizing-your-repositorys-social-media-preview
- Citation files: https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-citation-files
- Funding links: https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/displaying-a-sponsor-button-in-your-repository
- Archiving repositories: https://docs.github.com/repositories/archiving-a-github-repository/archiving-repositories

## Checklist

### Community Profile

- README explains value, mechanics, commands, workflow, support, security, and license.
- Public repos have `LICENSE`, `CODE_OF_CONDUCT.md`, `CONTRIBUTING.md`, `SECURITY.md`, issue templates, PR template, and support route.
- Default community files are acceptable for organizations, but repo-local files should override defaults when the repo has special rules.

### Repository Identity

- Description is short, concrete, and aligned with the README first paragraph.
- Homepage points to real docs, demo, package page, or nothing. Do not use dead links.
- Topics cover language, framework, domain, audience, and product category without leaking private strategy.
- Social preview is recommended for public repos that will be shared.
- Screenshots or demo media are required only when users need to inspect UI, gameplay, CLI output, generated artifacts, or visual results.

### Contribution Routing

- Prefer GitHub issue forms for public repos because they collect structured information.
- Use `.github/ISSUE_TEMPLATE/config.yml` with `blank_issues_enabled: false` when unstructured issues create support noise.
- Route security reports to `SECURITY.md`, not public issues.
- Route support questions to discussions, docs, or support links when bug reports are not the right place.

### Maintenance State

State the repo status when it changes:

- `experimental`
- `active`
- `maintenance-only`
- `deprecated`
- `archived`
- `internal-only`

For deprecated or archived repos, update the README top section and repository description before archiving. Close or explain open issues and PRs where practical.

### Governance

Add `GOVERNANCE.md` or `MAINTAINERS.md` when the repo has more than one maintainer group, external contributors, foundation stewardship, or sensitive release decisions. For private repos, document owner team, business owner, technical owner, escalation path, and decision rules.

### Release Surface

- Public reusable repos should have a release policy.
- Package repos should have tags, changelog/release notes, and version rules.
- Skill repos should use lightweight SemVer tags plus reviewed GitHub Releases when users may pin or compare behavior.
- Internal apps usually need deployment notes rather than public-style version releases.

### Optional Signals

Use only when they apply:

- `CITATION.cff` for research, datasets, academic software, or work likely to be cited.
- `.github/FUNDING.yml` for public projects that accept sponsorship.
- `docs/status.md` or `MAINTENANCE.md` for support windows, deprecation timelines, or operational status.

## Avoid

- Badge soup.
- Fake "open source" posture without a license.
- Vague security policies that tell users to open public issues for vulnerabilities.
- Empty governance files.
- Stale screenshots and dead demo links.
- Default issue templates that do not collect reproduction detail.
- Topics, docs, or examples that reveal private customer, client, or strategy details.

## Verification Commands

```bash
gh repo view OWNER/REPO --json description,homepageUrl,isArchived,isTemplate,repositoryTopics,latestRelease,url,visibility
gh api repos/OWNER/REPO/community/profile
gh api repos/OWNER/REPO/labels
gh api repos/OWNER/REPO/releases --jq '.[0] | {tag_name,name,draft,prerelease,published_at}'
```

Report gaps as:

- Trust blocker: license missing on public repo, no security reporting route, misleading archived/deprecated state, or visibility mismatch.
- Polish gap: no social preview, no screenshots where needed, weak topics, no release/changelog for reusable public repo.
- Nice-to-have: citation, funding, or governance only where the profile fits.
