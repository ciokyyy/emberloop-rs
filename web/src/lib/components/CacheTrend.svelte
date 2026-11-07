<script lang="ts">
  import type { CachePoint } from '$lib/cache';
  import { percentLabel } from '$lib/cache';

  // The plot is drawn in a fixed viewBox and scales with its column; the axes sit in the margins.
  const W = 640;
  const H = 200;
  const M = { top: 14, right: 14, bottom: 34, left: 44 };
  const plotW = W - M.left - M.right;
  const plotH = H - M.top - M.bottom;
  const ticks = [0, 25, 50, 75, 100];

  let { points = [] }: { points?: CachePoint[] } = $props();

  const x = (index: number) =>
    points.length <= 1 ? M.left + plotW / 2 : M.left + (index * plotW) / (points.length - 1);
  const y = (percent: number) => M.top + plotH * (1 - percent / 100);
  const coords = $derived(points.map((point, index) => ({ x: x(index), y: y(point.percent), point })));
  const line = $derived(coords.map((c, i) => `${i === 0 ? 'M' : 'L'}${c.x.toFixed(1)},${c.y.toFixed(1)}`).join(' '));
  const first = $derived(points[0]);
  const last = $derived(points.at(-1));
  const desc = $derived(
    first && last
      ? `${points.length} requests from #${first.seq} to #${last.seq}: ${percentLabel(first.percent)} at the first, ${percentLabel(last.percent)} at the latest.`
      : 'No requests with a hit rate yet.'
  );
</script>

<svg viewBox="0 0 {W} {H}" class="block h-auto w-full" role="img" aria-labelledby="cache-trend-title cache-trend-desc">
  <title id="cache-trend-title">Prompt-cache hit rate per request</title>
  <desc id="cache-trend-desc">{desc}</desc>
  <text x={M.left - 8} y={M.top - 4} text-anchor="end" class="fill-fg-3 text-[10px]">Hit %</text>
  {#each ticks as tick (tick)}
    <line x1={M.left} x2={W - M.right} y1={y(tick)} y2={y(tick)} class="stroke-border" stroke-width="1" />
    <text x={M.left - 6} y={y(tick)} dy="0.32em" text-anchor="end" class="fill-fg-3 text-[10px] tabular-nums">{tick}</text>
  {/each}
  <line x1={M.left} x2={M.left} y1={M.top} y2={M.top + plotH} class="stroke-fg-4" stroke-width="1" />
  {#if first}
    <text x={M.left} y={H - 16} text-anchor="start" class="fill-fg-3 text-[10px]">#{first.seq}</text>
  {/if}
  {#if last && last !== first}
    <text x={W - M.right} y={H - 16} text-anchor="end" class="fill-fg-3 text-[10px]">#{last.seq}</text>
  {/if}
  <text x={M.left + plotW / 2} y={H - 3} text-anchor="middle" class="fill-fg-3 text-[10px]">Request, oldest to newest</text>
  {#if coords.length > 1}
    <path d={line} fill="none" class="stroke-status-working" stroke-width="2" stroke-linejoin="round" />
  {/if}
  {#each coords as c (c.point.seq)}
    <circle cx={c.x} cy={c.y} r="2.5" class="fill-status-working">
      <title>#{c.point.seq}: {percentLabel(c.point.percent)}</title>
    </circle>
  {/each}
</svg>
