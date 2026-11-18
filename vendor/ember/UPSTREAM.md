# Ember skill pack: upstream sources

Ember is the emberloop-rs development pipeline: `ember-flow` routes to `ember-grill`, `ember-design`, `ember-audit`, `ember-slice`, `ember-build` and `ember-review`. `scripts/install-ember` copies each folder here into `~/.agents/skills/`. This folder is the source of truth; edit here, then reinstall.

Upstream packs are merged by hand. To update, read the upstream change, port what fits into the ember skill that owns that idea, and update the commit below.

| Source | Version | License | Used in | How |
| --- | --- | --- | --- | --- |
| obra/superpowers, https://github.com/obra/superpowers | commit 22f0b46 | MIT, copyright Jesse Vincent | `ember-build` (test-driven-development, systematic-debugging, verification-before-completion), `ember-review` (receiving-code-review) | Rules rewritten in `SKILL.md`. `ember-build/references/` holds unmodified copies of `root-cause-tracing.md`, `defense-in-depth.md`, `condition-based-waiting.md`, `condition-based-waiting-example.ts`, `find-polluter.sh`, `writing-good-tests.md` and the license as `LICENSE-superpowers`. |
| Google Elephant-Goldfish Model, https://github.com/google/elephant-goldfish-model | commit 72d06f52a2482885d9a64bd24b291c6e8628942c (plugin 0.1.1) | Apache-2.0 | `ember-audit` (egm-review, egm-review-loop, egm-resolve), `ember-design` (egm-draft and its design template) | `ember-audit/agents/`, `resources/document_standards/`, `LICENSE` are unmodified; `SKILL.md` files are modified rewrites. See `ember-audit/NOTICE`. |
| Matt Pocock's skills, https://github.com/mattpocock/skills (`grilling`, `grill-with-docs`, `domain-modeling`, `to-tickets`, `implement`, `code-review`, `diagnosing-bugs`, `tdd`) | commit 5863806 (prior skills plus `diagnosing-bugs` and `tdd`) | no license file shipped | `ember-grill`, `ember-slice`, `ember-build` (ticket loop, debugging loop, tautological-test rule), `ember-review` | Ideas only; text rewritten. |
| Andrej Karpathy skills (`karpathy-guidelines`), https://github.com/ForrestChang/andrej-karpathy-skills | `main` (`6a62d04`) | MIT | `ember-build` (surgical changes, simplicity), `ember-review` (dead-code scope) | Ideas only; text rewritten. |

Emberloop-specific parts: one long-lived elephant Session that uses `compress` at stage and ticket boundaries instead of a fresh Session, goldfish personas spawned with `spawn role=goldfish`, the persistent `spec` and `standards` reviewer pair, numbered question rounds instead of one question per turn, and tier `mid` by default.
