# Ticket 16: Context tree v2

## Goal

Rework `core/context.rs` into the Cordis-shaped model of spec decision N: typed services (`provide`, `get`, `require`), `Plugin` with `deps()` and `apply`, RAII `Scope` with reverse-order disposers, events with `emit`, `parallel`, `bail` and `serial`, child Contexts that override chosen services (a Session is one), extension-point registries (Harness, ToolProvider, ModelSource, Compressor, PromptSource, RouteContributor), and a fake harness plugin. Re-register the existing agy driver, tools, proxy and prompts as plugins. Blocked by ticket 02; do it before ticket 04.

## Status

Not done. `crates/core/src/context.rs` is 51 lines with `provide` and `get` only: no `Plugin` with `deps`, `Scope`, event modes, child Contexts or extension registries. The daemon builds its services directly and never uses `Context`.

## Tests first

Failing before implementation: a plugin with unmet `deps` is not applied, is applied when the service appears, is disposed when it goes away and re-applied when it returns; disposers run in reverse order and a disposed scope's event listeners are gone; `bail` stops at the first `Some` and `serial` keeps order; a child Context overrides one service and shares the rest; a panicking plugin does not take down the tree; a fake harness is added with one `ctx.plugin` line and no change to any core crate; the core crate stays under about 600 lines.
