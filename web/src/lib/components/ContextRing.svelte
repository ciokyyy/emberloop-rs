<script lang="ts">
  import { contextPct, formatTokens, meterTone } from '$lib/contextMeter';

  let { used = null, size = 0 }: { used?: number | null; size?: number } = $props();

  const R = 8;
  const C = 2 * Math.PI * R;
  const pct = $derived(contextPct({ used: used ?? 0, size }));
  const percent = $derived(pct === 0 && used !== null && used > 0 ? '<1%' : `${pct}%`);
  const label = $derived(
    used === null ? 'Context usage unavailable' : size > 0 ? `Context ${percent} · ${formatTokens(used)} of ${formatTokens(size)} tokens` : 'Context window unknown'
  );
</script>

<span
  class="inline-flex items-center gap-1.5 rounded-full px-1.5 py-1 text-[12px] tabular-nums {meterTone(pct)}"
  title={label}
  aria-label={label}
  role="img"
  data-testid="context-ring"
>
  <svg viewBox="0 0 20 20" class="h-[18px] w-[18px] -rotate-90" aria-hidden="true">
    <circle cx="10" cy="10" r={R} fill="none" stroke="currentColor" stroke-opacity="0.2" stroke-width="2.5" />
    {#if size > 0 && used !== null}
      <circle
        cx="10"
        cy="10"
        r={R}
        fill="none"
        stroke="currentColor"
        stroke-width="2.5"
        stroke-linecap="round"
        stroke-dasharray={C}
        stroke-dashoffset={C * (1 - pct / 100)}
        style="transition: stroke-dashoffset var(--dur-emphasis) var(--ease-smooth)"
      />
    {/if}
  </svg>
  <span>{size > 0 && used !== null ? percent : '–'}</span>
</span>
