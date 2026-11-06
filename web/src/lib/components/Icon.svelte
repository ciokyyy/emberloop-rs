<script lang="ts" module>
  type Shape = { d?: string[]; circles?: [number, number, number][]; rects?: [number, number, number, number, number][] };

  // Lucide outlines, inlined: the tool feed needs a dozen glyphs and no icon package.
  const ICONS: Record<string, Shape> = {
    'chevron-right': { d: ['m9 18 6-6-6-6'] },
    'chevron-down': { d: ['m6 9 6 6 6-6'] },
    terminal: { d: ['M4 17 10 11 4 5', 'M12 19h8'] },
    pencil: {
      d: [
        'M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z',
        'm15 5 4 4'
      ]
    },
    globe: { circles: [[12, 12, 10]], d: ['M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20', 'M2 12h20'] },
    bot: { d: ['M12 8V4H8', 'M2 14h2', 'M20 14h2', 'M15 13v2', 'M9 13v2'], rects: [[4, 8, 16, 12, 2]] },
    'file-text': {
      d: [
        'M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z',
        'M14 2v5a1 1 0 0 0 1 1h5',
        'M10 9H8',
        'M16 13H8',
        'M16 17H8'
      ]
    },
    search: { circles: [[11, 11, 8]], d: ['m21 21-4.3-4.3'] },
    list: { d: ['M3 5h.01', 'M3 12h.01', 'M3 19h.01', 'M8 5h13', 'M8 12h13', 'M8 19h13'] },
    lightbulb: {
      d: [
        'M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5',
        'M9 18h6',
        'M10 22h4'
      ]
    },
    'arrow-up': { d: ['m5 12 7-7 7 7', 'M12 19V5'] },
    square: { rects: [[4, 4, 16, 16, 2]] },
    paperclip: {
      d: [
        'm16 6-8.414 8.586a2 2 0 0 0 2.829 2.829l8.414-8.586a4 4 0 1 0-5.657-5.657l-8.379 8.551a6 6 0 1 0 8.485 8.485l8.379-8.551'
      ]
    },
    x: { d: ['M18 6 6 18', 'm6 6 12 12'] },
    sparkles: {
      d: [
        'M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z'
      ]
    }
  };

  export type IconName = keyof typeof ICONS;
</script>

<script lang="ts">
  let { name, size = 14, class: className = '' }: { name: IconName; size?: number; class?: string } = $props();
  const shape = $derived(ICONS[name] ?? {});
</script>

<svg
  xmlns="http://www.w3.org/2000/svg"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  class={className}
  aria-hidden="true"
>
  {#each shape.d ?? [] as d, index (index)}<path {d} />{/each}
  {#each shape.circles ?? [] as [cx, cy, r], index (index)}<circle {cx} {cy} {r} />{/each}
  {#each shape.rects ?? [] as [x, y, width, height, rx], index (index)}<rect {x} {y} {width} {height} {rx} />{/each}
</svg>
