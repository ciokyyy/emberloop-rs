# Auto layout engine quirks

Depth behind the rules in SKILL.md. Read when a frame's geometry does not match what `node.set` asked for.

## Coordinates

- `node.create`/`frame.create` with a `parent` take `x`/`y` relative to that parent. Passing page coordinates silently adds the parent's origin: a card asked for at page `1589,2849` inside a stage at `1045,2549` lands at `2634,5398`. Convert first, then confirm with `absoluteBounds`.
- `node.reparent` takes `{ids, parent, index}` only. Position after reparenting.
- `node.get {id, depth:0}` returns `absoluteBounds` plus `id/name/type/width/height/x/y/clip/sizing/layout/childCount`. The `layout` object carries `mode`, `gap`, `crossGap`, `padding`, `mainAlign`, `crossAlign`, `contentAlign`, `wrap`, `strokesIncluded` - that is the fastest way to see why a child sits where it does.

## Batch

- Steps are `{command, params, as}`; the params nesting is required, and a flat object fails with `unknown arguments commands[0].name`.
- Reference an earlier step with `{"$ref": "stepName"}`; a bare string is read as a node id and rejected.
- Reference an existing layer by its raw id.
- `edit.duplicate {ids, dx, dy, map:true}` answers `{ids, map}` where `map` is a flat original-id to copy-id dictionary covering descendants. Keep it: the next edits address the copies by those ids, and a copy's tree can always be re-read by name.
- The MCP call wraps those steps: `figcraft.batch {commands:[{as, command, params}]}`. Keep the list short, about a dozen steps: a hundred-step call can roll back wholesale with no failing entry to read, so build one tile or row at a time.
- With `onError:"continue"` each entry comes back as `{step, command, ok, result}`; without it they are bare results. Check `ok` after every batch, and parse the answer when it arrives as a JSON string instead of a list.

## Sizing

- A hug frame grows and shrinks with its children: shrinking a 24x24 close box to 14x14 collapsed a 36 px header to 32 px and moved every sibling. Resize the child inside its box, not the box.
- `sizingH`/`sizingV` accept `fixed`, `hug`, `fill`. `fill` fills the parent's CONTENT box, which excludes padding and, with `strokesIncluded:true`, the inside border.
- `crossAlign:"center"` and `mainAlign` operate inside that content box, so asymmetric padding offsets anything centred on the cross axis. Prefer symmetric padding plus a border on a wrapper frame.
- `strokesIncluded:false` keeps the border out of the content box, which is what makes a full-height tab sit flush against a strip's bottom hairline.

## Paint

- New frames carry a default white fill; new text and rectangles take the fill you pass. Any wrapper meant to be invisible needs `fill:"none"`.
- A frame with `layout:null` stacks children at their own `x`/`y`; a frame with a layout mode positions them from the layout and ignores those values. Absolute stacking (a ring track under its arc) needs `layout:null`.

## Verifying a cropped composition

Export the stage alone at `scale:2`, then walk one column and one row and print the colour runs. A run table proves the bleed, the gutter and the surface steps at once:

- the ground colour from `0` to the hero's top inset;
- the hero surface from there to the image's last row (a bleed ends on the hero's colour, a gutter ends on the ground);
- the right-hand gutter on a horizontal walk at the hero's mid-height.

Sample every element that matters by `absoluteBounds * scale` - bar segments, the active underline, a satellite's fill and its leader - because a stage that renders is not a stage that renders correctly.
