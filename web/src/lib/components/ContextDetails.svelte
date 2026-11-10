<script lang="ts">
  import { api } from '$lib/api';
  import type { ContextBreakdown, UsageRecord } from '$lib/api/types';
  import { percentLabel, sessionHit } from '$lib/cache';
  import { formatTokens } from '$lib/contextMeter';
  import Button from './ui/button.svelte';

  let { used = null, size = 0, breakdown = null, oncache, sessionId = '' }: {
    used?: number | null; size?: number; breakdown?: ContextBreakdown | null; oncache?: () => void;
    sessionId?: string;
  } = $props();
  const categories = [
    ['system', 'System', '#e496f6'],
    ['toolDefinitions', 'Tool definitions', '#88bcff'],
    ['toolCalls', 'Tool calls', '#81c9bb'],
    ['toolResults', 'Tool results', '#90ca90'],
    ['compression', 'Compression', '#e2b623'],
    ['conversation', 'Conversation', '#fd92bd']
  ] as const;
  const count = (value: number) => Math.round(value).toLocaleString();

  // The ring reads the Session's cache hit rate, which the feed does not carry: fetch the usage rows.
  const HISTORY = 200;
  let rows = $state<UsageRecord[]>([]);
  $effect(() => {
    const id = sessionId;
    let live = true;
    if (!id) { rows = []; return; }
    api.sessionUsage(id, HISTORY)
      .then((loaded) => { if (live) rows = loaded; })
      .catch(() => {});
    return () => { live = false; };
  });
  const hit = $derived(sessionHit(rows));
  const R = 14.5;
  const C = 2 * Math.PI * R;
</script>

<div class="flex flex-col gap-4">
  <div class="flex items-center justify-between gap-3 rounded-xl border border-border bg-surface-0 p-3">
    <div class="min-w-0">
      <p class="text-[10px] font-semibold uppercase tracking-[0.05em] text-fg-3">Reported input</p>
      {#if used === null}
        <p class="mt-1 text-[17px] text-fg-2">Not reported yet</p>
      {:else}
        <p class="mt-1 flex flex-wrap items-baseline gap-1.5 text-[17px] font-semibold tabular-nums">{count(used)}
          <span class="text-[12px] font-normal text-fg-3">of {size > 0 ? `${formatTokens(size)} tokens` : 'an unknown window'}</span></p>
      {/if}
    </div>
    <div class="flex shrink-0 flex-col items-end gap-2.5">
      <div class="flex items-center gap-2.5">
        <span class="inline-flex size-8 shrink-0 items-center justify-center" role="img" aria-label={`Prompt cache hit ${percentLabel(hit)}`} data-testid="cache-ring">
          <svg viewBox="0 0 32 32" class="size-8 -rotate-90" aria-hidden="true">
            <circle cx="16" cy="16" r={R} fill="none" stroke="var(--track)" stroke-width="3" />
            {#if hit !== null}
              <circle cx="16" cy="16" r={R} fill="none" stroke="var(--brand)" stroke-width="3" stroke-linecap="round"
                stroke-dasharray={C} stroke-dashoffset={C * (1 - hit / 100)} />
            {/if}
          </svg>
        </span>
        <div class="flex flex-col">
          <span class="text-[17px] font-semibold tabular-nums">{percentLabel(hit)}</span>
          <span class="text-[10px] font-semibold uppercase tracking-[0.05em] text-fg-3">Cache hit</span>
        </div>
      </div>
      {#if oncache}
        <Button onclick={() => oncache()}>Prompt cache</Button>
      {/if}
    </div>
  </div>
  {#if breakdown}
    <div>
      <h3 class="text-[13px] font-semibold">What is in context</h3>
      <p class="mt-1 text-[12px] text-fg-3">Estimated from the latest outgoing proxy request, after compression. Not an exact allocation of the provider total.</p>
    </div>
    <div class="flex h-2 overflow-hidden rounded bg-surface-3" aria-hidden="true">
      {#each categories as [key, , color] (key)}
        <span style:background={color} style:width={`${breakdown.estimatedTokens ? breakdown[key] / breakdown.estimatedTokens * 100 : 0}%`}></span>
      {/each}
    </div>
    <dl class="flex flex-col gap-2.5 text-[13px]">
      {#each categories as [key, label, color] (key)}
        <div class="flex items-center justify-between gap-3">
          <dt class="flex items-center gap-2 text-fg-2"><span class="size-2 rounded-full" style:background={color}></span>{label}</dt>
          <dd class="tabular-nums">≈ {count(breakdown[key])}</dd>
        </div>
      {/each}
      <div class="flex justify-between border-t border-border pt-2.5"><dt>Total</dt><dd class="font-semibold tabular-nums">≈ {count(breakdown.estimatedTokens)}</dd></div>
    </dl>
    {#if breakdown.imageCount}
      <p class="text-[12px] text-fg-3">{breakdown.imageCount} image(s) in context. Image token cost is not available; image bytes are excluded from the text estimate.</p>
    {/if}
    <p class="text-[11px] text-fg-3">Text estimates use UTF-8 bytes ÷ 4. Provider tokenizers, message framing and hidden overhead differ. Compression counts retained summaries, not the original folded text.</p>
  {:else if used !== null}
    <p class="rounded-xl border border-dashed border-border p-4 text-[13px] text-fg-3">Not available for this harness: the proxy never sees its requests, so there is no request body to break down.</p>
  {:else}
    <p class="rounded-xl border border-dashed border-border p-4 text-[13px] text-fg-3">Send a prompt to inspect the next request’s context breakdown.</p>
  {/if}
</div>
