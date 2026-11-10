# Components, variants and instances

Depth behind the reusable-components rules in SKILL.md. Read when building a components page, a variant set, or a control the app will reuse.

## Masters

- `node.create {type:"component", name, width, height, props:{…}}` creates a main component; the `props` are `node.set` keys, so layout, fill, stroke, padding and radius all apply.
- Pass `sizingH:"fixed"`, `sizingV:"fixed"` and both dimensions. A master with `fill` sizing created inside an auto-layout card takes the card's content box (a 263x42 tab landed as 256x40) and every instance of it is born that size. Reserve `fill`/`hug` for instances.
- A master may live inside a frame or a specimen card; nesting is fine and keeps the page readable.

## Variants

1. Name the mains before combining: `<Set>/<Property>=<Value>`, e.g. `Tab/State=Inactive` and `Tab/State=Active`. A name without `=` becomes `Property 1=Default`.
2. `component.combineVariants {ids}` answers `{id}` of the new COMPONENT_SET. The set is larger than the variants it holds (two 256x42 tabs became a 568x82 set: 20 px padding, 16 px gap), so resize and reposition any card that contains it and re-read the set's `absoluteBounds` afterwards.
3. `instance.create {component:<set id>}` instantiates the default variant. `instance.setProperties {ids, properties:{"Tab/State":"Active"}}` switches the variant; the property key is the set name plus the `=` name (`Tab/State`), and an unknown key fails with `unknown property \`State\` (known: Tab/State)`.
4. `edit.duplicate` of a main component yields an instance of it, not a second master — use that to populate a page quickly.

## Compound masters

A control made of other components (`Tabs` holding two `Tab` instances) is itself a `type:"component"` whose children are instances. Build it like any frame, then `instance.create` the children into it. Changing the set later updates the compound master and every instance of it.

## Specimen page

- One card per control class, x=40, width 560, `fill:#262626`, `stroke:#3B3B3B`, radius 16, padding 16, vertical layout, gap 12; a card that holds a variant set needs width 600 or more.
- A 10 px SemiBold `--fg-4` label with 5 % tracking sits 22 px above each card. No page title.
- A state whose underline or divider is the surface colour only reads correctly on a `#262626` card: an inactive tab underline painted `#262626` over a `#1B1B1B` page hides nothing and shows as a dark bar.
- Close with an `INSTANCES` card holding `instance.create` copies, so the page proves the linkage rather than describing it.
- After a card grows, reflow every section below it in one batch (`node.set {ids, y}`) and re-read the page's children: overlapping specimens look fine in a node report and only show in the export.
