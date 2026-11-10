# RFC: Code-Agent Memory Architecture & Evaluation for emberloop-rs

**Status**: Proposal / RFC for Agent & Human Review  
**Date**: 2026-11-09  
**Target Subsystems**: `crates/proxy`, `crates/tools`, `crates/daemon`

---

## 1. Executive Summary & Problem Definition

Standard LLM memory systems (e.g. Mem0, Letta/MemGPT, Zep, LangChain Memory) are designed for conversational chatbots and personal assistants. They store user profiles, preferences, and conversational turns, then retrieve them using semantic similarity (dense embeddings).

When applied to **coding agents** (autonomous software engineering agents navigating repositories, running builds, editing files, and running test suites), these systems suffer severe failure modes:
1. **Secondary Source Drift**: Memories recorded in March are loaded with equal confidence in June, long after code has been refactored or deleted.
2. **Distractor Degradation on Solvable Tasks**: Academic benchmarks (MemDev-Bench, REM) demonstrate that injecting unstructured memories into initial prompts causes **4.8% to 9.9% regression** on tasks the agent could otherwise solve without memory.
3. **Branch Desynchronization**: External vector databases or sidecars have no awareness of git branches. Memories learned on experimental or abandoned branches pollute main/feature branches.
4. **Token Inflation**: Injecting past trajectory traces consumes thousands of tokens per turn, accelerating context compaction and driving up latency/cost.

**The Goal**: Design a lightweight, zero-daemon, branch-aware memory mechanism in pure Rust that improves coding agent performance over long sessions without regressing solvable tasks or inflating context.

---

## 2. Research & Evidence Base

This design synthesizes findings from 6 state-of-the-art sources:

### A. TokenEconomizer (MSR / UC Berkeley — arXiv:2610.22732)
* **Target**: SWE-bench issue resolution.
* **Mechanism**:
  * **Micro-skills**: 1-sentence environmental facts extracted automatically from bash/build failures (e.g., `cargo test requires --all-targets`).
  * **Strict Budget**: Capped at $\le 4$ facts and $\le 150$ tokens.
  * **TurnGuard**: Loop detector that catches cyclical file edits or repeated command failures and injects active redirects.

### B. MemDev-Bench (arXiv:2610.27524, Oct 2026)
* Evaluated 29 memory engines across 3,331 historical pull-request tasks.
* **Key Finding**: General memory systems degraded overall resolution rates due to stale distractor facts.
* Specialized systems with strict deduplication and progressive disclosure (e.g. Claude-Mem) increased solve rate on hard tasks from **35.3% to 42.6%**.

### C. REM: "What to Store, When to Recall, What to Forget" (Amazon / Yale — arXiv:2610.27377)
* **Core Discovery: Struggle-Gated Recall**.
* Unconditional retrieval at prompt start distracts the agent. Past resolution experiences should only be surfaced when the agent **struggles** (failing test suite, non-zero compiler exit, repetitive patch failures).

### D. GitHub Copilot Agentic Memory (GitHub Blog, Jan 2026)
* **Core Discovery: Just-in-Time Code Citation Verification**.
* Memories must store explicit code references:
  ```json
  {
    "fact": "API version must match between client SDK, server routes, and docs",
    "citations": ["src/client/constants.ts:12", "server/routes/api.go:8"]
  }
  ```
* Before applying a memory, the system reads cited file lines. If citations point to nonexistent files or modified lines, the memory is discarded or self-healed.

### E. MemCoder (Tsinghua / Proxseer — arXiv:2603.13258, Feb 2026)
* Proved that structuring repository commit history into an actionable sextuple $(o_i, c_i, k_i, p_i, r_i, s_i)$—specifically pairing root-cause analysis with concrete diffs—boosted DeepSeek-V3.2 on SWE-bench Verified from 68.4% to 77.8%.

### F. Matt Pocock / AI Hero (`aihero.dev`)
* **Filesystem First**: The codebase is the primary source of truth. Memory is a secondary source. Keep memory in plain files in the environment so git branch switching handles state automatically.
* **Context Roles**: The implementer agent suffers extreme context pressure; the reviewer agent has a clean slate. Enforce standards in review and automated checks (lint, types), not through long prose in the implementer's prompt.
* **Progressive Disclosure**: Invalidate aggressively, prune no-ops, and use 1-line navigation pointers instead of inlining memory dumps.

---

## 3. Comparison Matrix: Approaches

| Dimension | Option 1: External Node Sidecar (e.g. agentmemory) | Option 2: Vector DB + Full Trajectories (Mem0 / Letta) | Option 3: Rust Filesystem Memory (Proposed) |
| :--- | :--- | :--- | :--- |
| **Dependencies** | Requires Node runtime, npm packages, sidecar daemon | Python/Node daemon, Chroma/Qdrant, vector embeddings | **Zero new dependencies**. Native Rust in proxy/tools |
| **Branch Awareness** | ❌ None (global external DB) | ❌ None (database detached from git) | ✅ **100% git-aware** (stored in `.agents/memory/`) |
| **Token Cost** | High (500–2,000 tokens per prompt) | High (unfiltered semantic recall) | **Capped at $\le 150$ tokens** for env, 0 on clean turns |
| **Staleness Handling** | Silent drift (outdated facts remain) | Hallucinated relevance | **Just-in-time citation check** against live disk files |
| **Trigger Mechanism** | Always injected on turn 1 | Always injected | **Struggle-gated** (injected only when errors occur) |
| **Maintenance** | High operational surface | High latency, cold start | Trivial (JSON/Markdown files inspected with git) |

---

## 4. Proposed Architecture for `emberloop-rs`

### 4.1 Storage & Layout
Memory is stored inside the project jail under `.agents/memory/`:

```
<project-root>/
├── .agents/
│   ├── skills/              # Procedural playbooks (P2-2 Hermes style)
│   └── memory/              # Epistemic facts & quirks
│       ├── env.json         # Micro-skills: build/test quirks (<150 tokens)
│       └── citations.json   # Code-anchored cross-file invariants
```

Because `.agents/memory/` sits in the project tree:
* Developers can choose to commit it to git (sharing repo quirks across a team) or gitignore it (local machine quirks only).
* Switching branches instantly syncs memory with that branch's exact environment.

### 4.2 Mechanism 1: Micro-Skills (`env.json`)
Micro-skills store 1-sentence environmental facts extracted from bash errors:
```json
[
  {
    "id": "cargo-test-targets",
    "fact": "cargo test requires --all-targets to run tests in tests/ directory.",
    "tool": "bash",
    "updated_at": "2026-11-09T00:52:00Z"
  }
]
```
- **Lifecycle**:
  1. Proxy observes a tool failure (e.g., `bash` exit code $\ne 0$) followed in later turns by a successful alternative.
  2. Background review fork (existing `crates/proxy/src/review.rs`) extracts a concise 1-sentence rule.
  3. Stored in `env.json`.
- **Injection**:
  - Inserted into system prompt under `<environment-memory>`.
  - **Strict Budget**: Max 4 items, $\le 150$ tokens total. Oldest or unverified items are automatically evicted.

### 4.3 Mechanism 2: Code Citations (`citations.json`)
Stores architectural invariants that span across multiple files:
```json
[
  {
    "id": "version-sync",
    "fact": "Cargo.toml version must match version constant in crates/api/src/state.rs",
    "citations": [
      "Cargo.toml:3",
      "crates/api/src/state.rs:15"
    ],
    "line_hashes": ["e3b0c442...", "28a6f01a..."]
  }
]
```
- **Just-in-Time Verification**:
  - When a citation is queried, the proxy computes the BLAKE3 hash of the cited lines on disk.
  - If lines shifted or files were deleted, the memory is invalidated and discarded immediately. It never hallucinates obsolete code structures.

### 4.4 Mechanism 3: Struggle-Gated Recall & TurnGuard
- **During normal execution**: Citations are **never** injected into the prompt. The agent works with a clean, unpolluted context window.
- **When struggle is detected**:
  - TurnGuard tracks:
    1. Same bash command failing $\ge 2$ times.
    2. Edit ping-pongs (file modified, reverted, and re-modified).
    3. Test failure status codes.
  - Proxy injects matching memories or a redirection steer into the user turn carrying tool results:
    ```
    <system-steer>
    TurnGuard: Detected 2 consecutive test failures in crates/proxy.
    Relevant verified memory: Cargo.toml version must match crates/api/src/state.rs:15.
    </system-steer>
    ```

---

## 5. Review & Evaluation Questions for Other Agents / Humans

When discussing this RFC with team members or other review agents, consider:

1. **Extraction Cost vs. Value**:
   * *Question*: Should micro-skills be extracted exclusively via the background review fork (`crates/proxy/src/review.rs`), or should the agent have an explicit `save_memory` tool?
   * *Recommendation*: Use the background review fork. An explicit tool encourages agents to spend turns saving trivia instead of writing code.

2. **Commit vs. Gitignore**:
   * *Question*: Should `.agents/memory/` be committed to git by default?
   * *Recommendation*: Add `.agents/memory/` to default project templates, but leave it to the user. `env.json` is useful across team CI; session-specific citations might be ephemeral.

3. **Scope Distinction (Skills vs. Memory)**:
   * **Skills (`.agents/skills/`)**: Multi-step *procedural playbooks* (how to conduct a retro, how to scaffold an exercise, how to review PRs). Read manually or via BM25 catalog.
   * **Memory (`.agents/memory/`)**: 1-line *epistemic facts and invariants* (build flags, version locks, struggle redirects). Injected with strict token caps or struggle triggers.

4. **Is it worth implementing now?**:
   * If the agent frequently fails initial build commands or gets stuck in repetitive edit loops on this repo, **Yes**.
   * If current runs are short and single-turn, current Hermes skills (`P2-2`) plus context compression already provide sufficient continuity.
