# Icons in a FigCraft frame

## Size: one 16 px box per icon

The app draws every UI glyph at 16 px (`h-4 w-4` / `size-4`). In a mock, a 16 px box holds a lucide 24-grid glyph, so its ink is 8-13 px wide, not 16: that proportion is correct, and a glyph whose ink fills the whole box reads as oversized next to the label. Give each icon a `16x16` box frame, never a bare vector.

## Path data resizes the layer

`node.set {path}` fits the vector layer to the path's own bbox in path units. A 24-grid glyph therefore lands about 24 px wide, and each icon comes back at a different size (measured 12x6 to 30x31 across one sidebar).

Wrapper recipe, per icon:

1. `node.create` a frame at the icon's own parent and index: `{width:16, height:16, fill:"none", layout:null, clip:false}`, name it `Icon box · <icon>`.
2. `node.reparent {ids:[vector], parent: box}`.
3. `node.set` the vector to `w = bboxW*16/24`, `h = bboxH*16/24`, `x = (16-w)/2`, `y = (16-h)/2`.

Do every icon of a page in one `figcraft.batch` (a 63-step batch runs fine and is atomic), so no icon is left half-wrapped.

## From lucide to one path string

Lucide ships node data, not a single `d`: `web/node_modules/@lucide/svelte/dist/icons/<name>.svelte` holds `__iconData = {...};`. Extract with `re.search(r'__iconData = (\{.*?\});', src, re.S)` and `json.loads(...)['node']`, a list of `[tag, attrs]`. Convert each element and join:

- `path` -> `d`
- `line` -> `M{x1} {y1}L{x2} {y2}`
- `circle` -> `M{cx-r} {cy}a{r} {r} 0 1 0 {2r} 0a{r} {r} 0 1 0 {-2r} 0`
- `rect` -> `M{x} {y}h{w}v{h}h{-w}Z`
- `polyline` / `polygon` -> `M` + `L`.join(points)

Stroke: `strokeWeight:2`, `strokeAlign:"center"`, `fill:"none"`, `strokeCap:"round"`, `strokeJoin:"round"`.

### A concatenated subpath that starts with `m`

A relative moveto is relative to the start of its own `<path>`. After concatenation it continues from the previous subpath's end point: `M18 6 6 18` + `m6 6 12 12` draws down to y=36 and the layer bbox becomes 18x30 instead of 12x12. Prefix such a subpath with an absolute `M0 0` (`M0 0m6 6 12 12`) and never uppercase the `m` - that makes the following implicit lineto absolute and shifts the glyph.

## Icons the app draws inline

When a component writes its own `<svg>` instead of using lucide (sidebar tabs, sliders, pickaxe), copy that `d` string verbatim from the component source so the mock and the code agree. Keep its own `strokeWidth` (the sidebar inline set uses 1.75).

## Check the size without eyes

Export the frame holding the icons at a high scale, decode the PNG with `zlib` (filter types 0-4), and take the ink bbox inside each icon box: it must equal the glyph bbox * 16/24, centred. A box that reports 16x16 while its ink spans 20 px means the vector was never wrapped or its scale was set from the wrong bbox.
