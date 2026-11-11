# Audit several boards for consistency

Run this when the user says the boards must be consistent, or before turning several approved screens into code. Compare values, not impressions.

## Read the tree with an explicit field allowlist

`figcraft.get_node` rejects fields outside its allowlist: `props` and `padding` both fail with an `unknown field` error. `padding` and `gap` live under `layout`, colours are hex strings in `fills[0].color` / `strokes[0].color`.

```python
F = ["id","name","type","characters","width","height","layout","fills","strokes",
     "fontSize","fontStyle","cornerRadius","strokeWeight","textAlign","sizing","clip"]

def col(v):
    return v[0].get("color") if v else None

rows = []
def walk(n, screen):
    ly = n.get("layout") or {}
    rows.append(dict(s=screen, id=n["id"], type=n.get("type"), w=n.get("width"), h=n.get("height"),
        fs=n.get("fontSize"), fw=n.get("fontStyle"), ch=(n.get("characters") or "")[:28],
        fill=col(n.get("fills")), stroke=col(n.get("strokes")), r=n.get("cornerRadius"),
        gap=ly.get("gap"), pad=tuple(ly.get("padding") or []), mode=ly.get("mode")))
    for c in n.get("children") or []:
        walk(c, screen)

for sid, label in screens.items():
    walk(tools.call("figcraft.get_node", id=sid, depth=20, fields=F), label)

from collections import Counter
print(Counter((r["fs"], r["fw"]) for r in rows if r["type"] == "TEXT").most_common())
print(Counter((r["s"], r["gap"], r["pad"]) for r in rows if r["mode"] == "vertical" and r["w"] == 720))
```

Read every node's `layout`, because `padding` is not a node field: compute the body's gap and padding from `n["layout"]`. Also re-read a frame's own `x`/`y` only for placement, never as proof of spacing.

## Values that must match across screens of one app

- Body frame: 720x663 at (280,57), `#1B1B1B`, vertical, `gap:16`, `padding:[16,16,16,16]`, `clip:true`. A board at `gap:12` is drift, not a variant.
- Header row 56 px plus a 1 px `#3B3B3B` divider: title 14 px SemiBold `#FFFFFF`, subtitle 12 px `#9E9E9E`.
- Toolbar 36 px with a 12 px `#9E9E9E` caption left and one secondary button right.
- Cards and rows: fill `#262626`, 1 px `#3B3B3B`, radius 12, `strokesIncluded:false`; row height 56, padding `[8,14]`, title 13 `#D4D4D4`, description 12 `#9E9E9E`, 1 px dividers.
- Type scale in use: 14/13/12/11/10 px, Regular plus SemiBold only (Inter has no Medium). A 9 px label is acceptable only inside the compact ring variant; anywhere else it is drift.
- Section headings: pick one level per app — 14 px SemiBold card heading, or the 10 px SemiBold eyebrow — and do not mix both on one page family.

## Acting on a mismatch

- Fix in place with `node.set` after `object.toggleLock {ids, locked:false}` on the affected top-level screen; never delete and redraw a locked approved board.
- A deliberate difference (a compact or alternative variant) is legitimate: record it in `design.md` in one line so the next audit does not "fix" it.
- Re-lock and `file.save`, wait past the 5 s snapshot, then re-read one node per page to confirm `locked:true`.
