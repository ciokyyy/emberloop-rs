## What changed

<!-- One paragraph: what moves, and why. -->

## Verification

<!-- Quote the command you ran and its result line. -->

```
scripts/check
```

## Checklist

- [ ] The change keeps its scope: no unrelated files reformatted or touched.
- [ ] `scripts/check` (or `scripts/check-changed`) passes; no stage was weakened.
- [ ] A behaviour change has a test that fails without it.
- [ ] `AGENTS.md` and user-facing docs are updated where a contract moved.
- [ ] `web/build` is rebuilt and committed if `web/` source changed.
- [ ] Commit messages follow `type(scope): summary`.

Closes #
