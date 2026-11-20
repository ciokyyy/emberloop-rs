# Skill usage audit

Use when the question is whether skills (or the retro that writes them) are read, useful, or too coarse. Answer consumption and quality first; cost is secondary evidence.

## Procedure

1. Get the facts with `emberloop-daemon digest --since <unix ms> --limit <N> --out <dir>` (`<data root>/current/emberloop-daemon` when it is not on `PATH`). It reads live `events/*.jsonl` and archived `events.zstd` alike. Done when the stdout line `shown M of N` covers the window you need; raise `--limit` or move `--since` until it does.
2. Split skill writes by origin. Writes through `edit`/`write` on `el://skills/<global|project>/<name>/...` paths log `create`, `patch` or `write_file` rows in the skill root's `.usage.jsonl`. Disk-path writes log nothing; the digest lists them under unattributed skill changes. Older history also holds `compression` events with action `skill-review` (the removed background review) and `tool_started` calls of the removed skill-write tool. Skills with no record are human or vendored.
3. Take loads from the digest's skill stats ("reads seen": `.usage.jsonl` `view` lines, `SKILL.md` reads only). Count reads of `references/`, `scripts/` and `resources/` files separately from the tool calls in each Session digest, because asset-rich skills otherwise look over-read.
4. Take exposures from the same table. A skill inside a module shows only the module line in a Session's catalog, so the digest counts it as module-listed; report module-listed and named skills as separate groups. Read rate = reads seen / exposures, per origin group.
5. Judge quality on a sample of 5-10 loads: did the later turns follow the rule (count the behaviour before and after the change, e.g. `status=$?` in bash calls)? Then check granularity: SKILL.md bytes and heading count, growth per change, changes that repeat a rule the skill already has, `description:` edits (the trigger tightens only through these), and contradictions between skills.
6. Find silent deletions: a skill name in old catalogs or events but in no skill root. Its text survives in `~/.agents/skills/.backup/`.
7. Add cost: a retro is a normal Session, so its `usage` events carry its whole cost. Report retro spend per change it made.
8. Compare against the cheapest alternative producer, not against nothing: skills the main agent created itself, or no writer at all. Score the writer against the gates below. Done when the report names keep, slim or replace, and states each open question with its options in `.scratch/<slug>/decisions.md`.

## Gates a skill writer should pass

These come from TokenEconomizer's learning rules (`microsoft/TokenEconomizer`, MIT). Judge any skill writer against them.

- Evidence: create a rule only from a pattern seen in 3 or more Sessions. A one-Session lesson is usually a narrative.
- Size: keep SKILL.md to the always-on rules and move depth into `references/`. An append-only writer grows a skill without bound. The User prefers a writer skill that teaches this over a mechanical byte cap in the write path.
- Retirement: drop a rule that later Sessions contradict, and flag a skill that stays unread across many exposures.
- Reversibility: keep a record of every change, so a retired skill can be traced and restored.
- Own cost: count the writer's model calls in the cost the User sees.
- Trust: a writer that replays untrusted tool output must not write durable instructions unattended. Let the User approve, or run it as a visible Session.

## Limits to state in the report

- Loads through `tools.read` inside `execute`, `cat` in bash, or skill text pasted into a `spawn` brief log no `view` line, so real usage is higher. The ranking is unaffected.
- A high read rate on recently changed skills is confounded: a writer tends to change the skills that Sessions already open.
- The catalog caps at 50 lines, so a skill past the cap shows 0 exposures.
