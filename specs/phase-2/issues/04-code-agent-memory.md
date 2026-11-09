# P2-4: Code-Agent Filesystem Memory (Micro-skills, Code Citations, TurnGuard)

## Status: Spec Draft (2026-11-09)

## Goal

Equip `emberloop-rs` with lightweight, branch-aware, filesystem-based memory specialized exclusively for coding agents. Avoid generic conversational memory, vector database daemons, and token bloat.

Synthesizes:
- **Matt Pocock (aihero.dev)**: Filesystem-backed state, secondary-source drift mitigation, progressive disclosure, navigation pointers over raw memory dumps.
- **TokenEconomizer (MSR / UC Berkeley, arXiv:2610.22732)**: Micro-skills (1-sentence environmental facts extracted from bash/build failures, strict token cap).
- **GitHub Copilot Agentic Memory (2026)**: Just-in-time code citation verification against disk.
- **REM (Amazon / Yale, arXiv:2610.27377)**: Struggle-gated recall (retrieve experience only when stuck).

---

## 1. Filesystem Layout

Memory lives directly in the project directory, branch-aware via git:

```
<project-root>/.agents/
├── skills/                     # Procedural playbooks (P2-2 Hermes style)
└── memory/                     # Code & environment memory
    ├── env.json                # Micro-skills: build quirks, env flags
    └── citations.json          # Code facts anchored to file:line references
```

No external databases, no background sidecars. Pure Rust file I/O inside `crates/proxy` and `crates/tools`.

---

## 2. Core Mechanisms

### A. Micro-Skills (Environment & Build Quirks)
- **Path**: `.agents/memory/env.json`
- **Schema**:
  ```json
  [
    {
      "id": "cargo-test-flags",
      "fact": "Run cargo test with --all-targets to include integration tests.",
      "observed_tool": "bash",
      "updated_at": "2026-11-09T00:52:00Z"
    }
  ]
  ```
- **Injection Policy**:
  - Injected into system prompt under `<environment-memory>`.
  - Hard cap: Max 4 facts, maximum 150 tokens total.
  - Least-recently-updated entries are dropped when capacity exceeded.

### B. Just-in-Time Code Citations
- **Path**: `.agents/memory/citations.json`
- **Schema**:
  ```json
  [
    {
      "subject": "version-sync",
      "fact": "Cargo.toml version must match version in src/lib.rs",
      "citations": ["Cargo.toml:3", "src/lib.rs:12"],
      "content_hash": "blake3-hash-of-cited-lines"
    }
  ]
  ```
- **Validation on Load**:
  - Before injecting or searching citations, the proxy checks the cited files and line ranges.
  - If cited files or line hashes diverge, the citation is flagged stale or pruned. No secondary source drift.

### C. Struggle-Gated Recall & TurnGuard
- **Trigger**:
  - Normal turn: Citations are NOT dumped into prompt (preserves context budget and prevents distraction on solvable tasks).
  - Struggle condition detected:
    - Same bash command fails $\ge 2$ times with non-zero exit.
    - File edit reverted or cyclic ping-pong edits detected across turns.
    - Test execution fails.
- **Action**:
  - The proxy injects matching memories or a TurnGuard steering redirect into the next prompt steer.

---

## 3. Integration Points in emberloop-rs

1. `crates/proxy/src/session.rs`: Injects `<environment-memory>` (max 150 tokens) during prompt preparation.
2. `crates/proxy/src/review.rs`: Background review fork extracts candidate micro-skills when bash errors are resolved.
3. `crates/tools/src/lib.rs`: Registers memory inspect/edit tools if explicit agent management is enabled.
