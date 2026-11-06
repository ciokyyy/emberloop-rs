<script lang="ts">
  import type { FeedItem } from '$lib/feed';
  import { collapse } from '$lib/motion';
  import { toolAppearance } from '$lib/tools';

  let {
    item,
    open = false,
    ontoggle
  }: {
    item: FeedItem;
    open?: boolean;
    ontoggle: () => void;
  } = $props();

  const appearance = $derived(toolAppearance(item.tool));
  const lines = $derived((item.body ?? '').split('\n'));
</script>

<div>
  <button class="trow" aria-expanded={open} onclick={ontoggle}>
    <span class="glyph {appearance.className}">{appearance.glyph}</span>
    <span class="min-w-0 flex-1 truncate font-mono text-[13px]">
      {item.summary || item.tool || 'tool'}
    </span>
    <span class="hide-sm max-w-[45%] truncate text-[12px] text-fg-3">
      {item.meta || (item.finished ? '' : 'running')}
    </span>
    <span class="text-fg-3 transition-transform {open ? 'rotate-90' : ''}">▸</span>
  </button>

  {#if open}
    <div class="tbody" transition:collapse>
      <pre>{#if item.tool === 'edit'}{#each lines as line, index (index)}{#if line.startsWith('+')}<span class="add">{line}</span>{:else if line.startsWith('-')}<span class="del">{line}</span>{:else}{line}
{/if}{/each}{:else if item.tool === 'bash' || item.tool === 'background'}<span class="text-fg-3">$ {item.summary}</span>
{item.body || '(no output yet)'}{:else}{item.body || '(no content yet)'}{/if}</pre>
    </div>
  {/if}
</div>
