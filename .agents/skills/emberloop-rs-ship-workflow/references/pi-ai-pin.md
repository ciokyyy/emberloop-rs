# Updating the pinned pi-ai dependency

`pi-ai` is not a crates.io release: `crates/daemon/Cargo.toml` pins a commit in the `ciokyyy/pi-rust` fork. Upstream is `yannelli/pi-rust`; the fork carries the `ruzstd` security fix that upstream has not merged.

## Check for an update

- List upstream tags: `git ls-remote --tags https://github.com/yannelli/pi-rust | tail -30`. Tags sort lexically, so confirm the numeric maximum instead of taking the last line.
- List fork branches: `git ls-remote --heads https://github.com/ciokyyy/pi-rust`.
- Read the current pin in `crates/daemon/Cargo.toml` and its base version in `crates/daemon/AGENTS.md`; the patch commit message names the base tag.

## Rebuild the patch on a new upstream tag

- `git clone https://github.com/ciokyyy/pi-rust /tmp/pi-rust-fork`
- `cd /tmp/pi-rust-fork && git remote add upstream https://github.com/yannelli/pi-rust`
- `git fetch upstream refs/tags/<tag>:refs/tags/upstream-<tag>`
- `git checkout -B fix/<topic>-<tag> upstream-<tag>`
- `git cherry-pick <existing-patch-commit>`. Resolve only the version context in `Cargo.toml`/`Cargo.lock`.
- `git diff upstream-<tag> HEAD -- Cargo.toml Cargo.lock` must show only the intended dependency and `rust-version` bump.
- `cargo check -p pi-ai` in the fork before pushing.

## Publish and repin

- Push the branch with the credential rule from `SKILL.md`: `git -c credential.helper='!gh auth git-credential' push -u origin fix/<topic>-<tag>`.
- Read the full SHA from the remote: `git ls-remote https://github.com/ciokyyy/pi-rust refs/heads/fix/<topic>-<tag>`. Never construct or guess a full SHA from a short one.
- Set that SHA as the `pi-ai` rev in `crates/daemon/Cargo.toml`.
- Run `cargo update -p pi-ai`; it moves `pi-ai`, `pi-schema` and `pi-telemetry` to the new rev in `Cargo.lock`.
- Update the `pi-ai` sentence in `crates/daemon/AGENTS.md` with the new upstream version and full commit.
- Run `cargo check --workspace --all-targets`, then `scripts/check`; commit the pin, lock and doc together.

## Pitfalls

- Upstream may still depend on vulnerable `ruzstd 0.8` after a release bump; the patch moves the workspace dependency to `0.9.1` and raises `rust-version` to `1.87`. `0.9.1` cannot satisfy a `^0.8` requirement, so a `[patch.crates-io]` entry does not fix it; the git dependency itself must be patched.
- A fork branch based on an old tag misses later upstream work. Always branch the patch from the newest tag and keep the diff limited to the dependency files.
- Check the pin's base version, not only the fork's default branch: a fork commit can sit on a side branch while `main` still carries the vulnerable dependency.
