#!/usr/bin/env bash
set -euo pipefail

target="${1:-.}"

if [[ ! -d "$target" ]]; then
  echo "Target is not a directory: $target" >&2
  exit 2
fi

cd "$target"

echo "== Git status =="
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git status --short
  echo
  echo "== Remotes =="
  git remote -v || true
else
  echo "Not a git worktree"
fi

echo
echo "== Secret-like filenames =="
find . \
  -path './.git' -prune -o \
  -type f \( \
    -name '.env' -o -name '.env.*' -o -name '*secret*' -o -name '*credential*' \
    -o -name '*token*' -o -name '*.pem' -o -name '*.key' -o -name 'id_rsa*' \
    -o -name 'service-account*.json' -o -name '*firebase*.json' \
  \) -print | sed 's#^\./##' || true

echo
echo "== Secret-like content markers =="
if command -v rg >/dev/null 2>&1; then
  rg -n --hidden --glob '!.git/**' --glob '!node_modules/**' --glob '!dist/**' --glob '!build/**' \
    '(AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]+|xox[baprs]-[A-Za-z0-9-]+|-----BEGIN (RSA |OPENSSH |EC |DSA )?PRIVATE KEY-----|PRIVATE_KEY=|DATABASE_URL=|SUPABASE_SERVICE_ROLE|STRIPE_SECRET|GOOGLE_APPLICATION_CREDENTIALS)' \
    . || true
else
  echo "rg not installed; skipped content marker scan"
fi

cat <<'EOF'

Review any findings before publishing. This is a lightweight preflight, not a guarantee.
For high-risk public releases, also use a dedicated scanner such as gitleaks/trufflehog
and review git history before pushing.
EOF

