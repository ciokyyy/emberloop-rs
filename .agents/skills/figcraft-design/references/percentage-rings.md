# Percentage rings and per-family usage tiles

Depth behind the usage-view rules in SKILL.md. Read when drawing a circular percentage meter or a per-family usage board.

## Ring geometry

A ring is a stack, so its frame needs `layout:null` (see `auto-layout.md`): with an auto-layout mode the engine repositions the track and the arc apart.

Three sizes, same stack; the arc radius is the track's radius, so it always lands on the track's centreline.

| ring | frame | track | arc `r`, `cx:cy` | percent text |
|------|-------|-------|------------------|--------------|
| large | 56x56 | ellipse 52x52 at (2,2), radius 26 | r 26, 28:28 | 11 px SemiBold, `y:21`, `width:56` |
| compact | 40x40 | ellipse 36x36 at (2,2), radius 18 | r 18, 20:20 | 10 px SemiBold, `y:14`, `width:40` |
| dense | 32x32 | ellipse 28x28 at (2,2), stroke 3, radius 14 | r 14, 16:16 | 9 px SemiBold, `y:11`, `width:32` |

- Track props: `{fill:"none", stroke:"#3E3E3E", strokeWeight:4, strokeAlign:"center"}`; the dense ring uses `strokeWeight:3` on both track and arc.
- Arc: `vector` created with `path` already in ring-local coordinates, and NO `width`/`height` (passing them scales the path box non-uniformly and drops the arc off the track).
- Percentage text: 11 px SemiBold `#FFFFFF` at `x:0`, `y:21`, `width:56`, `textAlign:"center"`, `textAutoResize:"none"`. Place it by that fixed centre, never from the returned text height — a text node's box height is not its line box.

## Arc path

Start at the top centre `(cx, cy-r)` and sweep clockwise to the value:

| view | value |
|------|-------|
| start | `M cx cy-r` |
| end angle | `-90 + pct*3.6` degrees |
| end point | `cx + r*cos(angle)`, `cy + r*sin(angle)` |
| arc | `A r r 0 <large> 1 <endX> <endY>` with `large = 1` only when `pct > 50` |

The `1` before the end point is the clockwise sweep flag — a `0` draws the short way round and the ring reads inverted. A half ring (`r:24, cx:cy:28`) is `M 28 4 A 24 24 0 0 1 28 52`.

Arc props: `{fill:"none", stroke:<colour>, strokeWeight:4, strokeAlign:"center", strokeCap:"round"}`.

## Traps

- A 100 % arc is rejected (`path` draws nothing): the start and end points coincide. Cap the drawn value below about 99 and keep the label truthful.
- Never resize a vector box or re-set its `path` after creation — it shifts the arc half a stroke off the track. Delete the arc and create it again.
- Colour by value, and keep one meter's ring, bar and percent text on the same token: green `#90CA90` under 70 %, amber `#E2B623` from 70 %, red `#FF9890` from 90 %.

## Grouped card board (one card per provider)

The current shape, and the one to draw first: a row of hug cards, each carrying every window that provider reports.

- Row: `fill:"none"`, horizontal, gap 12, `crossAlign:"min"`, `sizingH`/`sizingV` hug. At three cards (3, 4 and 2 windows) with the dense ring it measures about 534x99 in the 688 px content width — the leftover is fine, do not stretch the cards.
- Card: `#262626`, 1 px `#3B3B3B`, radius 12, `clip:true`, vertical, gap 10, padding 10.
- Head: horizontal hug group, gap 6, centred — 18x18 `#2F2F2F` radius 5 chip with the provider mark resized to 14x14 at `(2,2)`, then the provider name 11 px SemiBold `#FFFFFF`.
- Rings: horizontal hug group, gap 8, one 44 px cell each — dense 32 px ring, then its label 9 px Regular `#D4D4D4` centred at the cell's width, `textAutoResize:"none"`.
- Keep a cell label to one short word (`5h`, `Week`, `Fable`, `Gemini`, `Argon`): 44 px at 9 px holds about six characters, so `Claude 5h` wraps and makes that card taller than its siblings — name the card for the provider and let the label name only the window.
- The board was drawn one-card-per-window first and the user preferred this denser grouped shape; when asked for more compact, drop to the dense ring and single-digit padding, never add a column.
- A provider's mark fills its head chip and its families reuse it; a family with no mark of its own takes the provider's (Argon sits on Gemini's mark until it earns one).

Behind an equal-height row: set each card `sizingV:"fill"` in a `crossAlign` row that stretches, or leave all cards on the same number of ring rows and the same single-line labels. After any copy edit, re-read the row and each card's height — a stale wrapped label shows up as one card 15 px taller.

## One card per window (alternative)

- 220x122 tiles, three per 688 px row, gap 14, rows `sizingV:"hug"` so the row is uniform.
- Tile: `#262626`, 1 px `#3B3B3B`, radius 12, `clip:true`, vertical, gap 10, padding 12 — a head row (`#2F2F2F` 24x24 radius 8 chip, brand mark 20x20 inside at 2,2, then a 12 px SemiBold family name) over a 64 px meter row (ring, then window 11 px `#9E9E9E` over reset 10 px `#6B6B6B`).
- Use it only when each window needs its own reset line; the grouped card wins when the question is how a provider's pools compare.

## Column board (windows left to right)

The bar alternative: one 220 px card per provider, gap 14, all with `sizingV:"fill"` so the tallest column sets the height of its siblings. Each window is a 196 px meter — label over `NN% used`, a 4 px `#3E3E3E` track with a rounded fill as wide as the percent, then a 10 px `#6B6B6B` reset caption.

## Verify

Export at `scale:2` and sample three points per ring: the centre (`#262626` tile fill), a point on the track well away from the arc (`#3E3E3E`), and a point on the arc mid-sweep (the meter colour). A ring whose arc is missing leaves the track colour where the arc should be.
