# AGENTS.md — specs

## Purpose

Phase 1 specification, architectural decision records, issue breakdown, and UI prototypes. Prototypes are static HTML, not the daemon's web build: served over Tailscale at `https://agents.koboscintilla.com` (Cloudflare reroute to the Tailscale subdomain DNS), never on a public bind.

## Child DOX Index

- `specs/phase-1/issues/AGENTS.md` — phase 1 ticket definitions and progress status
- `phase-1/diagram.html` — three figures: the parts and their wires, one steered turn, and the ticket build order
- `phase-2/AGENTS.md` — phase 2: pi-rust and claude harnesses, subagents, `execute`, MCP client, Tauri
- `phase-1/mock/sidebar.html` — sidebar prototype: Project and New session on top, Sessions in the middle, Usage, Providers and Settings as an icon tray at the bottom; Lucide icons, Emberloop tokens
