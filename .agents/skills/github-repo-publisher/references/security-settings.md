# GitHub Repo Security And Governance Settings

Use this for private/internal repos and for public repos that accept contributions.

## Official Sources

- Rulesets: https://docs.github.com/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets
- Available rules: https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets
- Branch protection: https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/managing-a-branch-protection-rule
- CODEOWNERS: https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners
- Security policy: https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/adding-a-security-policy-to-your-repository
- Secret scanning push protection: https://docs.github.com/code-security/secret-scanning/protecting-pushes-with-secret-scanning
- Dependabot quickstart: https://docs.github.com/code-security/getting-started/dependabot-quickstart-guide
- GitHub Actions token security: https://docs.github.com/actions/concepts/security/github_token
- Secure use of Actions: https://docs.github.com/en/actions/reference/security/secure-use
- OpenSSF Scorecard: https://github.com/ossf/scorecard
- OpenSSF GitHub configuration best practices: https://best.openssf.org/SCM-BestPractices/github/

## Rulesets Versus Branch Protection

Prefer rulesets when available because multiple rulesets can layer, enforcement can be active/disabled, and readers can view active rules. Use classic branch protection when plan/API support or simplicity makes it the better fit.

Baseline default branch rules:

- Require pull request before merging.
- For solo-maintainer or early public repos, do not require approving review or CODEOWNER review by default. GitHub does not allow authors to approve their own pull requests, so review requirements can deadlock a one-maintainer repo.
- Require at least one approving review only when a second reviewer actually exists.
- Require CODEOWNER review only when CODEOWNERS maps to real reviewers who can approve.
- Dismiss stale approvals when sensitive code changes, when appropriate and reviewers exist.
- Require status checks only after CI names are known.
- Require conversation resolution.
- Block force pushes.
- Block deletions.
- Require linear history if the team prefers squash/rebase flow.
- Include administrators only when the org expects admins to follow the same path.

Do not require unknown status checks before the first CI run has created real check names.

Use these starting templates:

- `templates/api/ruleset-main-light.json` for most solo-maintainer, early public, or low-risk repos.
- `templates/api/ruleset-main.json` for the same solo-friendly baseline with the historical template name.
- `templates/api/ruleset-main-strict.json` for mature public repos or sensitive private repos.
- `templates/api/ruleset-tags-release.json` for versioned release tags.

## CODEOWNERS

Place `.github/CODEOWNERS` unless the repo already has a stronger convention. Cover:

```text
* @org/default-owner
.github/workflows/ @org/platform-or-security
.github/ @org/platform-or-security
infra/ @org/platform-or-security
terraform/ @org/platform-or-security
src/auth/ @org/security
src/billing/ @org/billing
db/ @org/data
prisma/ @org/data
```

Adjust paths to the repo. Do not invent teams that do not exist without marking them as placeholders.

## Security Files

`SECURITY.md` should include:

- supported versions
- how to privately report vulnerabilities
- expected acknowledgement and remediation timelines
- what not to report publicly
- disclosure/coordinated release process

Never direct users to public issues for security vulnerabilities.

## Dependabot

Generate `.github/dependabot.yml` only for ecosystems present in the repo. Common entries:

- `github-actions` for `.github/workflows`
- `npm` for `package.json`
- `pip` for `requirements.txt`
- `pip` or `uv`/Python ecosystem for `pyproject.toml` as supported by Dependabot
- `docker` for Dockerfiles

Group updates where noise would be high. Use Dependabot secrets for private registries, not normal Actions secrets.

Always include `github-actions` when workflows exist so actions stay patched.

## GitHub Actions

For each workflow:

```yaml
permissions:
  contents: read
```

Elevate per job only when required. Avoid broad `write-all`. Pin third-party Actions to full commit SHAs for sensitive/private repos; at minimum pin to major versions for low-risk public repos and document the tradeoff.

Avoid workflows that execute untrusted pull request code with write tokens or secrets. Be especially careful with `pull_request_target`.

## Private/Internal Defaults

- Private by default for client, internal, proprietary, credential-adjacent, admin, and database repos.
- Internal only if every enterprise member can read the contents safely.
- Access via teams rather than individual exceptions.
- Disable forking unless justified.
- Keep base permissions low.
- Enable secret scanning and push protection where available.
- Review audit logs for visibility, ruleset, security, Actions, secret, invitation, and access changes.
- Add custom properties when the org uses them, such as owner team, service, environment, data classification, compliance scope, and lifecycle.

## Public Defaults

- License at root if intended as open source.
- `SECURITY.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, issue templates, PR template.
- OpenSSF Scorecard or equivalent periodic security posture check.
- CI required before merge once stable.
- Release notes and tags for versioned projects.

## Settings As Code

For one repo, prefer `gh api` payloads and read-back verification. For org-scale governance, consider Terraform GitHub provider, Probot Settings, or GitHub Safe Settings.

If settings-as-code is used, protect the settings file itself with CODEOWNERS and branch rules. A writable settings file can effectively become an admin-level control surface.

## Plan And Feature Caveats

Some security features depend on GitHub plan, organization policy, or licensing. If `gh api` or `gh repo edit` returns a plan/permission error, do not work around it silently. Report the limitation and leave a precise manual/admin follow-up.
