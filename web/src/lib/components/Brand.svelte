<script lang="ts">
  import { brandSvg, brandTint } from '$lib/brands';

  let { id, size = 16 }: { id: string; size?: number } = $props();
  const svg = $derived(brandSvg(id));
</script>

<!-- Fixed box so every logo lines up; the SVG fills the box. Unknown ids get a letter. -->
<span
  class="inline-flex shrink-0 items-center justify-center [&>svg]:size-full"
  style:width="{size}px"
  style:height="{size}px"
  style:color={brandTint(id)}
  aria-hidden="true"
>
  {#if svg}
    {@html svg}
  {:else}
    <span class="text-[10px] font-semibold uppercase text-fg-3">{id.slice(0, 1)}</span>
  {/if}
</span>
