# Baked-in MCP Subsystems & Project Tooling in emberloop-rs

## Baked-in vs User-Installed MCPs

Core subsystems (codebase memory graph, design system MCPs) must not rely on manual user configuration in `mcp.json`:

1. **Daemon Boot Ensure (`crates/daemon/src/`)**:
   - Call an `ensure(&config_root)` function at daemon initialization (pattern: `codegraph::ensure`).
   - Write server definitions (command, binary fallback via npm/cargo, cache directories, setup commands, included tools) if missing.
   - User overrides survive, but absence or invalid config is repaired with stable system defaults.

2. **Context Auto-Injection (`crates/tools/src/mcpclient.rs`)**:
   - Never force agents or models to compose internal session paths or project names.
   - Intercept server calls in `McpRegistry::arguments()` and inject project-derived parameters (e.g. matching project name from root path) automatically.

3. **Wire Prompt System Injection (`crates/proxy/src/wire.rs`)**:
   - Expose subsystem capabilities to proxy wire adapters (`anthropic`, `google`, `openai`) by conditionally injecting guidance text into the top-level system prompt when enabled.

## Project Drawer & Canvas Architecture

1. **Header Trigger vs Sidebar**:
   - Global left sidebar is reserved for cross-project navigation (sessions, settings, providers, skills).
   - Project-scoped tools belong in a right-side Project Drawer triggered by a `•••` or chevron button in the project header.
   - Keeps the UI extensible for additional project-level resources (git state, design files, env vars).

2. **Native Page Rendering Over Iframes**:
   - Render document ASTs / vectors directly in Svelte rather than embedding via `<iframe>`.
   - Eliminates postMessage overhead, sandbox permission boundaries, and viewport resizing friction.

3. **Floating HUD Pill**:
   - Overlay a floating HUD control over the canvas.
   - Binds agent execution status, session switcher, and inline prompt input directly to canvas state.
