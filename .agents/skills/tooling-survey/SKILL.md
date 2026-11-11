---
name: tooling-survey
description: Use when asked to research, compare or recommend a tool, library, MCP server, provider or browser stack, including "which of these should we use" and "find the lightest/fastest/cheapest option".
metadata:
  origin: agent
---

# Tooling survey

The user asked for a category (MCP servers, a model provider, a browser stack).
Survey that category first. A verdict for a different category is a non-answer,
however right it is; give the evidence for the asked category, then the verdict,
including a verdict of "none of these beat what we run today" with the numbers
behind it.

## Procedure

1. State the criteria as columns before searching: what the user optimises
   (debugging depth, RAM, tokens), plus anything this repo forces
   (installation, protocol, sandbox). Criteria decide the shortlist, so fix
   them first.
2. Read what the harness already supports before ranking. Which integration
   paths exist is a hard filter: a tool that needs a path the harness lacks is
   not a candidate, and a tool reachable by an existing path costs less than
   one that is not. `grep` the crates for the protocol and for the config file
   the harness reads; `AGENTS.md` under the owning crate states the shape.
3. Search primary sources: the project README, docs, CHANGELOG, source. Search
   the blog posts only for measurements, and label them as second-hand.
4. Query GitHub search directly for the fast-moving category; blog lists miss
   this year's projects. In `execute`, fetch with `urllib.request` from a
   Python script, or run a standalone `curl`. Piping Python through `bash` is
   refused.
5. Install and run the top two yourself before recommending. A verified
   command beats a README claim, and running it exposes the failure the docs
   omit. Measure what you can (process RSS, output size in bytes) and keep
   claims with their source.
6. Write the comparison as one table with a column per criterion and one row
   per candidate, then a single verdict sentence. Mark each number measured,
   claimed by the project, or second-hand.

## Pitfalls

- Do not stop at the category the user has already installed or the one you
  personally favour. Compare, then recommend; the user decides. The installed
  tool is one row of the table, never the conclusion: finding that what already
  runs is sufficient is a verdict, and a verdict needs the asked category's
  table in front of it. When the user names a category and you return evidence
  about a different one, they will ask again and the survey restarts.
- Fix the failure the user reported before recommending a replacement. The
  original symptom is often an environment or setup cause with no bearing on
  the comparison; state the comparison, then the smallest change that removes
  the symptom, so the verdict does not read as an excuse for it.
- Do not present a README benchmark as measured. Say which arm the number came
  from and when it was taken; vendor comparisons age fast.
- Do not recommend a wrapper when the wrapper's only job is to call a binary
  the harness can already call, unless it removes a real step (schema
  exposure, output shaping, process lifetime). Name the step it removes.
- Unfamiliar high-star repositories prove little on their own: check releases,
  binary size, and whether the claims are reproducible on this machine.
- Check the process count and RSS of the thing the candidate drives, not just
  the candidate. The runtime it spawns usually dominates memory.
- When the risk is a runaway process rather than steady state, measure under an
  enforced cap instead of watching a peak afterwards: put the process tree in
  its own cgroup (`memory.max`, `memory.swap.max=0`), then read `memory.peak`
  and `memory.events` (`oom_kill`). A cap that kills the tree at the limit is
  also the fix to recommend when a page or build can loop without bound.

See `references/integration-paths.md` for the paths a tool can be reached by
in this repo.
