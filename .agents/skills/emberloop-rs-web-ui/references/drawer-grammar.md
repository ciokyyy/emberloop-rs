# One drawer grammar for both rails

Both side panels of the app are the same object seen from two edges: a 280px rail over a dark scrim, slid in with the project's single standard curve, dismissed the same three ways. `design.md` at the repo root states the contract; `web/src/app.css` implements it. Keep the two in step — a component that writes its own duration or bezier is the defect.

## Tokens (`web/src/app.css`, beside the motion scale)

```css
--dur-instant: 100ms;
--dur-standard: 180ms;
--dur-emphasis: 320ms;
--ease-smooth: cubic-bezier(0.22, 1, 0.36, 1);
/* One drawer grammar: both rails share this width, scrim and motion. */
--drawer-width: 280px;
--drawer-scrim: rgb(0 0 0 / 0.5);
```

## Classes (inside `@layer components`)

```css
.drawer-scrim { position: fixed; inset: 0; z-index: 20; background: var(--drawer-scrim); }
.drawer-rail { position: fixed; inset: 0 0 0 auto; z-index: 30; display: flex; flex-direction: column;
  width: var(--drawer-width); border-left: 1px solid var(--border); background: var(--sidebar);
  transform: translateX(100%); transition: transform var(--dur-standard) var(--ease-smooth); }
.drawer-rail.open { transform: none; }
@media (max-width: 767.98px) {
  .drawer { position: fixed; inset: 0 auto 0 0; z-index: 30; width: var(--drawer-width);
    transform: translateX(-100%); transition: transform var(--dur-standard) var(--ease-smooth); }
  .drawer.open { transform: none; }
}
```

The left rail is static above 768px and only becomes `.drawer` below it; the right rail is `.drawer-rail` at every width. Both use `--sidebar` for the panel, `--border` for the seam, and `translate` only: no opacity, scale or spring on the panel, because a transform keeps the text crisp and matches between the two edges.

## Svelte side

- The scrim is a `<button class="drawer-scrim" aria-label="Close ..." onclick={onclose} transition:fadeAt>` rendered only while open; the fade comes from `$lib/motion` (`fadeAt`, `motionParams()`), which reads the tokens and returns zero duration under `prefers-reduced-motion`.
- The panel stays mounted and is marked `inert={!open}` plus `aria-hidden={!open}`, so focus cannot reach it while closed.
- Dismiss on three paths: scrim click, a 32px close icon button, and `<svelte:window onkeydown>` on `Escape`.
- A phone sheet (bottom drawer under 640px) is the same family: full width, `--dur-standard`/`--ease-smooth`, a scoped `@keyframes` slide-up, and it stacks its controls vertically because a bar cannot hold them.

## Check it in the browser

```bash
agent-browser set viewport 390 844
agent-browser open 'http://127.0.0.1:3000/p/<project>/design'
# open, then measure after the transition settles
agent-browser eval "(()=>{const b=[...document.querySelectorAll('button')].find(el=>el.getAttribute('aria-label')==='Open sessions'); b&&b.click(); return 'clicked'})()"
sleep 1
agent-browser eval "(()=>{const a=document.querySelector('aside.drawer'); const s=document.querySelector('.drawer-scrim'); const cs=getComputedStyle(a); return JSON.stringify({rail:[a.getBoundingClientRect().width, cs.transitionDuration, cs.transitionTimingFunction, cs.transform], scrim:s&&getComputedStyle(s).backgroundColor})})()"
```

Expect `[280, "0.18s", "cubic-bezier(0.22, 1, 0.36, 1)", "none"]` and `rgba(0, 0, 0, 0.5)`. Repeat for the other rail and for a desktop viewport; check `Escape`, the scrim and the close button each close the panel. Sampling in the same eval as the click reports the pre-click `matrix(1, 0, 0, 1, -280, 0)` and a null scrim.
