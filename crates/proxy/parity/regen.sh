#!/usr/bin/env bash
# Rebuilds the reference kernel at the commit in BASELINE and rewrites golden/*.json from it.
# Not part of scripts/check and not a runtime dependency. Needs git, node and npm.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
. "$here/BASELINE" 2>/dev/null || true
repo=$(grep '^repo=' "$here/BASELINE" | cut -d= -f2-)
commit=$(grep '^commit=' "$here/BASELINE" | cut -d= -f2-)
work="${PARITY_WORK:-$(mktemp -d)}"
git clone --quiet "$repo" "$work/src"
git -C "$work/src" checkout --quiet "$commit"
(cd "$work/src/kernel" && npm install --ignore-scripts --no-audit --no-fund >/dev/null && npx tsc -p .)
KERNEL_DIR="$work/src/kernel" node "$here/regen.mjs"
