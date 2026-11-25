<script lang="ts">
  import { PenTool, ArrowUpRight, X } from '@lucide/svelte';
  import { api } from '$lib/api';
  import { fadeAt } from '$lib/motion';
  import type { Project } from '../api/types';
  import Switch from './ui/switch.svelte';

  let { project, open, onclose, onupdate, onopendesign }: {
    project: Project;
    open: boolean;
    onclose: () => void;
    onupdate: (project: Project) => void;
    onopendesign: () => void;
  } = $props();

  let busy = $state(false);
  let error = $state('');

  async function toggle(name: string) {
    if (busy) return;
    const tools = project.tools.map((tool) =>
      tool.name === name ? { ...tool, enabled: !tool.enabled } : tool
    );
    busy = true;
    error = '';
    try {
      onupdate(await api.updateProject(project.id, { tools }));
    } catch (problem) {
      error = problem instanceof Error ? problem.message : String(problem);
    } finally {
      busy = false;
    }
  }
</script>

<svelte:window onkeydown={(event) => { if (open && event.key === 'Escape') onclose(); }} />

{#if open}
  <button class="drawer-scrim" type="button" aria-label="Close project tools" onclick={onclose} transition:fadeAt></button>
{/if}

<aside class="drawer-rail {open ? 'open' : ''}" aria-label="Project tools" aria-hidden={!open} inert={!open}>
  <!-- 56px, the same height and hairline as the page header beside it. -->
  <header class="flex h-14 shrink-0 flex-row items-center justify-between gap-1 border-b border-border px-3">
    <div class="min-w-0">
      <h2 class="truncate text-[13px] font-semibold text-fg-1">{project.name}</h2>
      <p class="truncate text-[11px] text-fg-3">{project.root}</p>
    </div>
    <button
      type="button"
      aria-label="Close"
      class="icon-btn shrink-0 text-fg-2 hover:bg-surface-2 hover:text-fg-1"
      onclick={onclose}
    >
      <X class="size-4" />
    </button>
  </header>
  <nav class="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
    <p class="px-3 pt-3 pb-1 text-[11px] font-medium text-fg-3">Tools</p>
    <button
      type="button"
      class="flex w-full min-w-0 items-center gap-2 rounded-[10px] px-2 py-1.5 text-left hover:bg-surface-3 focus-visible:bg-surface-3 focus-visible:outline-none"
      onclick={() => { onclose(); onopendesign(); }}
    >
      <PenTool class="size-4 shrink-0 text-fg-2" />
      <span class="min-w-0 flex-1 truncate text-[13px] text-fg-1">FigCraft</span>
      <ArrowUpRight class="size-4 shrink-0 text-fg-3" />
    </button>
    {#if project.tools.length}
      <p class="px-3 pt-3 pb-1 text-[11px] font-medium text-fg-3">MCP tools</p>
      {#each project.tools as tool (tool.name)}
        <div class="flex w-full min-w-0 items-center gap-2 rounded-[10px] px-2 py-1.5 hover:bg-surface-3">
          <span class="min-w-0 flex-1 truncate text-[13px] text-fg-1">{tool.name}</span>
          {#if tool.url}
            <a
              class="icon-btn shrink-0 text-fg-2 hover:text-fg-1"
              href={tool.url}
              target="_blank"
              rel="noopener"
              aria-label={`Open ${tool.name}`}
            >
              <ArrowUpRight class="size-4" />
            </a>
          {/if}
          <Switch
            label={`${tool.name} enabled`}
            checked={tool.enabled}
            disabled={busy}
            onchange={() => toggle(tool.name)}
          />
        </div>
      {/each}
      <p class="px-3 pt-1 text-[11px] text-fg-3">Tool changes apply to new Sessions.</p>
    {/if}
    {#if error}<p role="alert" class="px-3 pt-1 text-[11px] text-red-400">{error}</p>{/if}
  </nav>
</aside>
