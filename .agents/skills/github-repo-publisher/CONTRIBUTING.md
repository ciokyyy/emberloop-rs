# Contributing

Thanks for helping improve GitHub Repo Publisher.

## What Makes A Good Contribution

Good changes make the skill safer, clearer, or more useful when publishing real repositories. Strong contributions usually fit one of these categories:

- clearer repo publishing workflow in `SKILL.md`
- better public/private README guidance in `references/`
- safer GitHub settings, ruleset, or Actions templates
- small helper scripts with no unnecessary dependencies
- validation improvements that catch placeholders, risky settings, or missing repo files

## Local Workflow

```bash
git checkout -b feature/your-change
bash -n scripts/secret-preflight.sh
python3 -m py_compile scripts/readme-completeness.py
python3 scripts/readme-completeness.py README.md
```

If you change JSON templates, also run:

```bash
python3 - <<'PY'
from pathlib import Path
import json
for path in Path("templates/api").glob("*.json"):
    json.loads(path.read_text())
    print(f"ok {path}")
PY
```

## Pull Request Expectations

- Keep the change focused.
- Explain the repo publishing problem the change solves.
- Include before/after behavior when changing workflow guidance.
- Avoid adding heavyweight dependencies unless there is a strong reason.
- Do not include real secrets, internal repo names, customer data, private URLs, or credentials in examples.

## Documentation Style

Use direct, plain English. The skill should help agents publish complete repositories, not generate vague marketing copy. When adding README guidance, include both the value story and the technical mechanics.
