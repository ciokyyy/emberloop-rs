# Committing your hunks out of a shared dirty tree

When another agent's uncommitted work fills the checkout, `git add <file>` commits their hunks too, and `git commit -- <path>` is worse: it takes the whole working-tree file.

1. `git reset -q` to empty the index (this also unstages their staged artifacts; the content stays in the working tree).
2. Stage the files that are wholly yours: `git add <paths>`. Confirm each is yours alone with `git diff -- <path>`; a file where you edited one doc comment can carry their code hunks.
3. For a mixed file, build the patch from the real diff instead of typing one. In `execute`, run `git diff -- <path>`, split it at the `@@` lines, keep the diff header plus the one hunk whose body contains a marker string from your change, and `tools.write` the result to a scratch path such as `.scratch/patches/<name>.patch`.
4. `git apply --cached .scratch/patches/<name>.patch` for each patch, then `rm -rf .scratch/patches`. Verify with `git diff --cached --stat` (only your files and lines) and `git diff -- <path>` (their hunks still uncommitted).
5. `git commit -m "…"` with no paths, so the commit is exactly the index; split unrelated changes into separate commits while the files are still separate.

- Hand-written hunks fail with `error: corrupt patch at line N`: a genuinely empty context line still needs its leading space, and the header counts must match. Always regenerate from `git diff`.
- Generated artifacts (`openapi.json`, `web/src/lib/api/schema.d.ts`) are one file each and a fresh regeneration carries their hunks along with yours, so they cannot be split. Stage them whole and say in the reply that their pending schema changes ride in your commit; leaving them unstaged keeps the `generated` check stage red.
- Never `git add -A` or `git stash` here. Read `git status --short` for leftover staged changes of theirs (`MM` = index and working tree both differ) before committing.