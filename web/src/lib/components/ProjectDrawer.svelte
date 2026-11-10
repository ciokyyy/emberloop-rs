<script lang="ts">
  import { PenTool, ArrowUpRight, X } from '@lucide/svelte';
  import { fadeAt } from '$lib/motion';
  import type { Project } from '../api/types';

  let { project, open, onclose, onopendesign }: {
    project: Project;
    open: boolean;
    onclose: () => void;
    onopendesign: () => void;
  } = $props();
</script>

<svelte:window onkeydown={(event) => { if (open && event.key === 'Escape') onclose(); }} />

{#if open}
  <button class="drawer-scrim" type="button" aria-label="Close project tools" onclick={onclose} transition:fadeAt></button>
{/if}

<aside class="drawer-rail {open ? 'open' : ''}" aria-label="Project tools" aria-hidden={!open} inert={!open}>
  <header class="flex flex-row items-center justify-between gap-1 px-2 pt-3 pb-2">
    <h2 class="min-w-0 truncate text-[13px] font-semibold text-fg-1">{project.name}</h2>
    <button
      type="button"
      aria-label="Close"
      class="grid size-8 shrink-0 place-items-center rounded-lg text-fg-2 hover:bg-surface-2 hover:text-fg-1"
      onclick={onclose}
    >
      <X class="size-4" />
    </button>
  </header>
  <nav class="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
    <p class="px-3 pt-3 pb-1 text-[11px] font-semibold uppercase tracking-wider text-fg-3">Tools</p>
    <button
      type="button"
      class="sel flex w-full min-w-0 items-center gap-2 rounded-[10px] px-2 py-1.5 text-left"
      onclick={() => { onclose(); onopendesign(); }}
    >
      <PenTool class="size-4 shrink-0 text-fg-2" />
      <span class="min-w-0 flex-1 truncate text-[13px] text-fg-1">FigCraft</span>
      <ArrowUpRight class="size-4 shrink-0 text-fg-3" />
    </button>
  </nav>
</aside>
