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

  // FigCraft is a built-in gated server: an enabled Project entry is the whole switch.
  const figcraftOn = $derived(
    project.tools.some((tool) => tool.name === 'figcraft' && tool.enabled)
  );
  const otherTools = $derived(project.tools.filter((tool) => tool.name !== 'figcraft'));

  async function save(tools: Project['tools']) {
    if (busy) return;
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

  function toggle(tool: Project['tools'][number]) {
    void save(
      project.tools.map((other) =>
        other.name === tool.name ? { ...other, enabled: !other.enabled } : other
      )
    );
  }

  function toggleFigcraft() {
    const entry = project.tools.find((tool) => tool.name === 'figcraft');
    void save(
      entry
        ? project.tools.map((tool) =>
            tool.name === 'figcraft' ? { ...tool, enabled: !tool.enabled } : tool
          )
        : [...project.tools, { name: 'figcraft', url: '', enabled: true }]
    );
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
    <div class="flex w-full min-w-0 items-center gap-2 rounded-[10px] px-2 py-1.5 hover:bg-surface-3">
      <PenTool class="size-4 shrink-0 text-fg-2" />
      <span class="min-w-0 flex-1 truncate text-[13px] text-fg-1">FigCraft</span>
      <button
        type="button"
        class="icon-btn shrink-0 text-fg-2 hover:text-fg-1"
        aria-label="Open FigCraft"
        title="Open FigCraft"
        onclick={() => { onclose(); onopendesign(); }}
      >
        <ArrowUpRight class="size-4" />
      </button>
      <Switch
        label="FigCraft enabled"
        checked={figcraftOn}
        disabled={busy}
        onchange={toggleFigcraft}
      />
    </div>
    {#if otherTools.length}
      <p class="px-3 pt-3 pb-1 text-[11px] font-medium text-fg-3">MCP tools</p>
      {#each otherTools as tool (tool.name)}
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
            onchange={() => toggle(tool)}
          />
        </div>
      {/each}
    {/if}
    <p class="px-3 pt-1 text-[11px] text-fg-3">Tool changes apply to new Sessions.</p>
    {#if error}<p role="alert" class="px-3 pt-1 text-[11px] text-status-failed">{error}</p>{/if}
  </nav>
</aside>
