<script lang="ts">
  import type { ContextBreakdown } from '$lib/api/types';
  import { formatTokens } from '$lib/contextMeter';
  import Button from './ui/button.svelte';
  let { used = null, size = 0, breakdown = null, oncache }: {
    used?: number | null; size?: number; breakdown?: ContextBreakdown | null; oncache?: () => void;
  } = $props();
  const categories = [
    ['system', 'System prompts', '#ab9df2'],
    ['toolDefinitions', 'Tool definitions', '#69b7ea'],
    ['toolCalls', 'Tool calls', '#58c7b8'],
    ['toolResults', 'Tool results', '#a7c779'],
    ['compression', 'Compression summaries', '#e8b76a'],
    ['conversation', 'Conversation', '#ce90b6']
  ] as const;
  const count = (value: number) => Math.round(value).toLocaleString();
</script>

<div class="flex flex-col gap-4">
  <div class="rounded-xl border border-border bg-surface-1 p-4">
    <p class="text-[12px] text-fg-3">Provider-reported input · latest request</p>
    {#if used === null}
      <p class="mt-1 text-[15px] text-fg-2">Not reported yet</p>
    {:else}
      <p class="mt-1 text-xl font-semibold tabular-nums">{count(used)} <span class="text-[13px] font-normal text-fg-3">tokens</span></p>
    {/if}
    <p class="mt-1 text-[12px] text-fg-3">{size > 0 ? `${formatTokens(size)} context window` : 'Context window unknown'}</p>
    {#if oncache}
      <Button class="mt-3" onclick={() => oncache()}>Prompt cache</Button>
    {/if}
  </div>
  {#if breakdown}
    <div>
      <h3 class="text-[13px] font-medium">What is in context</h3>
      <p class="mt-1 text-[12px] text-fg-3">Estimated from the latest outgoing proxy request, after compression. Not an exact allocation of the provider total.</p>
    </div>
    <div class="flex h-2 overflow-hidden rounded-full bg-surface-3" aria-hidden="true">
      {#each categories as [key, , color] (key)}
        <span style:background={color} style:width={`${breakdown.estimatedTokens ? breakdown[key] / breakdown.estimatedTokens * 100 : 0}%`}></span>
      {/each}
    </div>
    <dl class="flex flex-col gap-2.5 text-[13px]">
      {#each categories as [key, label, color] (key)}
        <div class="flex items-center justify-between gap-3">
          <dt class="flex items-center gap-2 text-fg-2"><span class="h-2 w-2 rounded-full" style:background={color}></span>{label}</dt>
          <dd class="font-mono tabular-nums">≈ {count(breakdown[key])}</dd>
        </div>
      {/each}
      <div class="flex justify-between border-t border-border pt-2.5"><dt>Estimated text total</dt><dd class="font-mono">≈ {count(breakdown.estimatedTokens)}</dd></div>
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
