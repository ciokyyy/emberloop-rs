# Emberloop

Emberloop orchestrates AI coding Sessions across harnesses, proxies, and tools inside a Project.

## Language

**Subagent**:
A persistent child Session spawned by a parent Session to perform a specialized role and report back through parent steers.
_Avoid_: Child bot, helper agent

**Reviewer**:
A Subagent with role `spec` or `standards` that audits changes against a ticket and starting point; singleton per parent and role, using fixed role-specific model settings.

**Worker**:
A Subagent with full file-editing and shell capabilities that executes delegated implementation tasks.

**Scout**:
A read-only Subagent for codebase research and exploration whose file-editing and write tools are disabled and whose shell access runs through an exploration gate.

**Elephant**:
The main Session in the Elephant-Goldfish model: it holds the full project context, runs the review skills and spawns Goldfish reviewers, then decides what to do with their reports.

**Goldfish**:
A read-only, stateless, one-shot Subagent started with `spawn role=goldfish` and a persona. It sees only the artifacts it is pointed at, not the Elephant's conversation. It reports once through `submit`, is then stopped automatically, and cannot be steered or prompted again.

**Intelligence Tier**:
An abstract capability level (`smol`, `mid`, `top`) chosen by a parent when spawning a Worker, Scout or Goldfish, mapped in settings to a concrete model and reasoning effort.

**Fallback Ladder**:
An ordered list of model and reasoning-effort rungs assigned to a Reviewer role or Intelligence Tier, tried sequentially when a Subagent run fails with an error.

**Goldfish Tier Ladder**:
The `goldfish_tiers` Fallback Ladder an Intelligence Tier picks for a Goldfish, kept separate so reviewers can run a model family different from the Elephant's; an empty ladder inherits the matching `subagent_tiers` ladder.
