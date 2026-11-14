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

**Intelligence Tier**:
An abstract capability level (`smol`, `mid`, `top`) chosen by a parent when spawning a Worker or Scout, mapped in settings to a concrete model and reasoning effort.

**Fallback Ladder**:
An ordered list of model and reasoning-effort rungs assigned to a Reviewer role or Intelligence Tier, tried sequentially when a Subagent run fails with an error.
