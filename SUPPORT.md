# Support

emberloop-rs is maintained by one person on a best-effort basis. Use these
routes:

- **Bug**: open a [bug report](https://github.com/ciokyyy/emberloop-rs/issues/new?template=bug_report.yml).
  Include the tag or commit, the command you ran, and the output.
- **Feature request**: open a
  [feature request](https://github.com/ciokyyy/emberloop-rs/issues/new?template=feature_request.yml).
- **Question or setup problem**: open an issue with the question template fields
  filled in; read `README.md` first.
- **Security vulnerability**: follow [SECURITY.md](SECURITY.md). Never in a
  public issue.

## What to include in a report

- what you expected and what happened
- the exact command, or the UI page and the Session it concerns
- the daemon's stderr, and `--data-root` when it is not the default
- which harness the Session used (`agy`, `pi`, `claude`, or another ACP agent)
- whether it reproduces on a fresh `--data-root`

## What support is not

- A guarantee of response time.
- Help running an unsupported harness, a patched provider, or a modified build.
- Support for old tags after the on-disk layout has moved on.
