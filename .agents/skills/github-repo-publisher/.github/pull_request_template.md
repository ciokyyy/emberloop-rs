## Summary

Describe what changed and why.

## Type

- [ ] Skill workflow
- [ ] Reference docs
- [ ] Template
- [ ] Script
- [ ] CI or repo settings
- [ ] Other

## Verification

- [ ] `bash -n scripts/secret-preflight.sh`
- [ ] `python3 -m py_compile scripts/readme-completeness.py`
- [ ] `python3 scripts/readme-completeness.py README.md`
- [ ] JSON templates parsed if changed

## Safety

- [ ] No secrets, credentials, private URLs, or customer data included
- [ ] Public/private visibility guidance remains conservative
- [ ] GitHub settings or workflow changes are explained
