Skill review. Use only skill_manage, skill_search and read. Do not use any other tool, and do not continue the user's task.

Review the conversation above and update the skill library. Be ACTIVE: most sessions produce at least one skill update, even if small. A pass that does nothing is a missed learning opportunity, not a neutral outcome.

Target shape of the library: CLASS-LEVEL skills, each with a SKILL.md of always-on rules and a small references/ set of topical depth. Not a flat list of narrow one-session skills, and not an umbrella hoarding a references/ file per session. This shapes HOW you update, not WHETHER you update.

Only agent-origin skills change. A skill whose metadata origin is human or downloaded is never patched here: name the change you would make in your reply instead.

What a skill IS: the instructions for doing a class of task the most efficient and correct way, to THIS user's specifications: the procedure, the tools and commands that work, the order, the user's preferences for how the result should look, and the pitfalls that cost time. A future session should be able to follow it and produce what the user wants on the first try. Everything below is about writing that well:
  • Procedure first: the steps in the order they are done, with the concrete commands, tool calls, and decision points. Lessons and pitfalls attach to the step they affect.
  • A pitfall is a generalizable rule + one clause of WHY (the mechanism), imperative. Not a narrative of what happened this session.
  • No PR/issue numbers, dates, ticket IDs, or quoted user text as content. The rule must stand without the incident behind it. Keep a short quote ONLY when the quote itself is the clearest statement of the rule.
  • The same lesson learned twice is ONE rule. Before adding, search the skill (and its references/) for the rule already stated; strengthen or clarify it rather than appending a second copy.
  • Not a duplicate of what the environment already teaches: repo AGENTS.md files, tool schema descriptions, and other always-loaded context. A skill carries the WORKFLOW and the pitfalls; it does not restate the codebase map or a tool's parameter list.
  • Always-on rules (standing user preferences, gates that apply to every instance of the task) live in SKILL.md itself, whole. references/ is for depth that is only needed sometimes: a decision table, a recipe, a domain note, each file topical and reusable, never '<date>-<incident>.md'. Prefer extending an existing references/ file over creating one.
  • Fix the skill in place when it is wrong: edit the sentence that misled, do not append 'UPDATE: actually...' underneath it.

Signals to look for (any one of these warrants action):
  • User corrected your style, tone, format, legibility, or verbosity. Frustration signals like 'stop doing X', 'this is too verbose', 'don't format like this', 'why are you explaining', 'just give me the answer', 'you always do Y and I hate it', or an explicit 'remember this' are FIRST-CLASS skill signals, not just memory signals. Update the relevant skill(s) to embed the preference so the next session starts already knowing.
  • User corrected your workflow, approach, or sequence of steps. Encode the correction as a pitfall or explicit step in the skill that governs that class of task.
  • Non-trivial technique, fix, workaround, debugging path, or tool-usage pattern emerged that a future session would benefit from. Capture it.
  • A skill that got loaded or consulted this session turned out to be wrong, missing a step, or outdated. Patch it NOW.

Preference order. Prefer the earliest action that fits, but pick one when a signal above fired:
  1. UPDATE A CURRENTLY-LOADED SKILL. Look back through the conversation for skills you read this session. If one covers the new learning, patch that one first. Re-read its SKILL.md during this review (see Read-before-write below).
  2. UPDATE AN EXISTING UMBRELLA. If no loaded skill fits but an existing class-level skill does (find it with skill_search), patch it. Add a subsection, a pitfall, or broaden a trigger.
  3. ADD A SUPPORT FILE under an existing umbrella, with skill_manage and a write_file op. Use the folder that matches the kind of file: references/<topic>.md for topical depth needed only sometimes (a decision table, a reproduction recipe, provider quirks, condensed domain notes), named by TOPIC; templates/<name>.<ext> for starter files meant to be copied and modified; scripts/<name>.<ext> for statically re-runnable actions. Do not paste error transcripts; distill them to the rule. Add a one-line pointer in SKILL.md so future agents find the file.
  4. CREATE A NEW CLASS-LEVEL SKILL only when no existing skill covers the class. The name MUST be at the class level. It MUST NOT be a specific PR number, error string, feature codename, library-alone name, or a 'fix-X / debug-Y / audit-Z-today' session artifact. If the proposed name only makes sense for today's task, it's wrong: fall back to 1, 2 or 3.

Read-before-write (ENFORCED: skill_manage refuses otherwise). Before you patch an existing skill's SKILL.md, call read on that SKILL.md during this review. Before you overwrite or remove an existing supporting file, call read on that exact file. Content quoted earlier in the conversation does NOT count: your write must be based on what read just returned. Creating a new skill or adding a new supporting file needs no prior read. If a write is refused with a read-before-write error, call read for the named target once and retry the write once; do not loop.

User-preference embedding: when the user expressed a style, format, or workflow preference, the update belongs in the SKILL.md body, not just in memory. Memory captures who the user is and the current state of your operations; skills capture how to do this class of task for this user. When they complain about how you handled a task, the skill that governs that task needs to carry the lesson.

If you notice two existing skills that overlap, note it in your reply. Never delete a skill here.

Do NOT capture as skills (these become persistent self-imposed constraints that bite you later when the environment changes):
  • Environment-dependent failures: missing binaries, fresh-install errors, post-migration path mismatches, 'command not found', unconfigured credentials, uninstalled packages. The user can fix these; they are not durable rules.
  • Negative claims about tools or features ('browser tools do not work', 'X tool is broken', 'cannot use Y from execute_code'). These harden into refusals the agent cites against itself for months after the actual problem was fixed.
  • Session-specific transient errors that resolved before the conversation ended. If retrying worked, the lesson is the retry pattern, not the original failure.
  • One-off task narratives. A user asking 'summarize today's market' or 'analyze this PR' is not a class of work that warrants a skill.
  • Unresolved failures: if the session ended WITHOUT actually finding a working method, do NOT write those attempts up as a 'reliable workflow' or 'recommended approach'. Either say 'Nothing to save', or, only if you are independently confident of a real working alternative, capture ONLY that alternative. Never the dead ends, and never dressed up as best practice.
If a tool failed because of setup state, capture the FIX (install command, config step, env var to set) under an existing setup or troubleshooting skill, never 'this tool does not work' as a standalone constraint.

'Nothing to save.' is a real option but should NOT be the default. If the session ran smoothly with no corrections and produced no new technique, just say 'Nothing to save.' and stop. Otherwise, act.

Your final reply is one or two lines: what changed, or 'Nothing to save.'
