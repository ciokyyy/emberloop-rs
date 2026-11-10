# Design

The shared grammar for the app's surfaces. Code owns the values; this file owns
the contract, so a new rail or sheet copies the tokens instead of inventing a
duration.

## Drawers

There is one drawer grammar: `web/src/app.css` defines it, and every rail or
sheet uses those class names. Nothing hard-codes a duration, easing or width at
the call site.

| Piece | Where | Value |
| --- | --- | --- |
| Width | `--drawer-width` (`web/src/app.css`) | `280px`, both rails, all viewports |
| Scrim | `.drawer-scrim` | `--drawer-scrim` = `rgb(0 0 0 / 0.5)`, `z-index: 20` |
| Panel | `.drawer-rail` / `.drawer` | `--sidebar` background, side border, `z-index: 30` |
| Motion | `--dur-standard` + `--ease-smooth` | `180ms`, `cubic-bezier(0.22, 1, 0.36, 1)` |
| Dismiss | scrim click, close button, `Escape` | both rails behave the same |

- Left rail: `Sidebar` inside `<aside class="drawer">` in
  `web/src/routes/+layout.svelte`. Static on desktop, slides in from the left
  below 768 px (Tailwind's `md`), hamburger visible on the same breakpoint.
- Right rail: `web/src/lib/components/ProjectDrawer.svelte`, `aside.drawer-rail`,
  slides in from the right at every width and stays mounted with `inert` while
  closed.

Rules:

- A drawer slides only on `transform`, over `.drawer-scrim` faded with
  `fadeAt`/`motionParams()` from `web/src/lib/motion.ts`. No opacity on the panel
  itself, no scale, no spring, no per-component duration.
- Never write `0.5s`, `300ms` or a bezier literal in a component. If a surface
  needs a new step, add a `--dur-*` token and read it.
- Reduced motion wins: `@media (prefers-reduced-motion: reduce)` in `app.css`
  zeroes transitions, and `motionParams()` returns a zero duration so the end
  state lands at once.

## Sheets

The phone form of the agent pill (`FigcraftAgentHud.svelte`) is a bottom sheet,
not a drawer: full width, top corners rounded, rises with the same
`--dur-standard`/`--ease-smooth` pair (`.sheet` + `sheet-up` keyframes scoped to
`max-width: 639px`). Anything that anchors to an edge uses these tokens too.

## Motion scale

| Token | Value | Use |
| --- | --- | --- |
| `--dur-instant` | `100ms` | hover/tint feedback |
| `--dur-standard` | `180ms` | drawers, sheets, colour and background transitions |
| `--dur-emphasis` | `320ms` | one-off emphasis (meters, larger moves) |
| `--ease-smooth` | `cubic-bezier(0.22, 1, 0.36, 1)` | the only easing |

## Mocks

`design.figcraft` (repo root) holds the 1:1 FigCraft mocks: the chat composer,
`Project tools · right rail` (280 px, `#171717`) and `Agent · phone drawer`.
The Skills screens (`Skills · desktop`, `Skill · desktop`) sit on the same shell:
the chrome sidebar with its Skills tab active, the session header as the top bar,
and a 720 px body — the list groups skills by scope, a row opens the detail.
The design editor (`Editor · desktop`, `Editor · phone`) puts the agent in a docked
340 px rail (`#171717`) instead of a corner pill: header, feed with the last turns,
and a composer card with a context chip. `Rail · collapsed` is the 48 px slim state;
on a phone the same rail is a bottom sheet over the canvas.
`Skill · editor` is the editing state of the skill detail: same shell and header, a
toolbar with the file name, a dirty dot and Discard/Save, then the code card
(`#171717`, mono 12, gutter, active-line `#262626`) and a line-count status row.
Change the mock and the code in the same pass, and keep this file's table true.
