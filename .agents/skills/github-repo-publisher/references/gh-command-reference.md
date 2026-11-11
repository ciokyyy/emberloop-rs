# GitHub Command Reference

Compact command patterns for this skill. Prefer these over ad hoc shell strings.

## Auth And Context

```bash
gh auth status
git status -sb
git remote -v
git branch --show-current
git fetch --tags --prune
```

## Create / Edit

```bash
gh repo create OWNER/REPO --private --description "..." --homepage "..." --source=. --remote=origin --push
gh repo edit OWNER/REPO --description "..." --homepage "..." --enable-issues --delete-branch-on-merge
gh repo edit OWNER/REPO --add-topic "topic-one,topic-two"
gh repo edit OWNER/REPO --visibility public --accept-visibility-change-consequences
```

## Read-Back Verification

```bash
gh repo view OWNER/REPO --json nameWithOwner,visibility,description,homepageUrl,repositoryTopics,defaultBranchRef,isPrivate,url
gh api repos/OWNER/REPO --jq '{visibility,has_issues,has_wiki,has_discussions,delete_branch_on_merge,allow_squash_merge,allow_merge_commit,allow_rebase_merge}'
gh api repos/OWNER/REPO/topics --jq '.names'
gh api repos/OWNER/REPO/rulesets --jq 'map({name,enforcement,target})'
```

## API Payloads

Use files for complex API payloads:

```bash
gh api repos/OWNER/REPO/rulesets --method POST --input templates/api/ruleset-main-light.json
gh api repos/OWNER/REPO/rulesets --method POST --input templates/api/ruleset-main-strict.json
```

For PR bodies and release notes, prefer body files over inline shell strings so backticks and Markdown are not interpreted by the shell.

## Release

```bash
gh release list --limit 10
gh repo view OWNER/REPO --json defaultBranchRef,latestRelease,pushedAt
git tag --list v0.1.0
git ls-remote --tags origin v0.1.0
git rev-parse HEAD
gh release create v0.1.0 --draft --generate-notes --title "v0.1.0"
gh release view v0.1.0 --json tagName,name,isDraft,isPrerelease,publishedAt,url,targetCommitish
gh workflow run release.yml --repo OWNER/REPO --ref main -f version=0.1.0
gh run watch RUN_ID --repo OWNER/REPO --exit-status
```

Create release notes from a file when Markdown contains backticks, command
substitution characters, or long verification lists.

## Safety

- Use explicit visibility flags.
- Verify before and after changing visibility.
- Do not inline secrets into command arguments.
- Prefer `--json` plus `--jq` for read-back checks.
