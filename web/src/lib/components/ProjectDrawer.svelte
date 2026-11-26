<script lang="ts">
  import { onMount } from 'svelte';
  import {
    Aperture,
    ArrowUpRight,
    AudioLines,
    FileText,
    FileType,
    Grid3x3,
    Image,
    LayoutTemplate,
    Palette,
    PenTool,
    Plug,
    Presentation,
    Ruler,
    Shapes,
    Sparkles,
    Spline,
    Table,
    X,
  } from '@lucide/svelte';
  import { api } from '$lib/api';
  import { fadeAt } from '$lib/motion';
  import type { CraftApp, Project } from '../api/types';
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
  let craft = $state<CraftApp[]>([]);

  onMount(async () => {
    try {
      craft = await api.listCraft();
    } catch (problem) {
      // Keep the built-in FigCraft row reachable when the list fails.
      craft = [{ name: 'figcraft', label: 'FigCraft', summary: '', url: '', web: '', installed: true }];
      error = problem instanceof Error ? problem.message : String(problem);
    }
  });

  // Each Craft app gets its own mark; every other server keeps a generic one.
  const marks: Record<string, typeof PenTool> = {
    figcraft: PenTool,
    cadcraft: Ruler,
    craftanything: Shapes,
    deckcraft: Presentation,
    designcraft: LayoutTemplate,
    effectcraft: Sparkles,
    gridcraft: Table,
    lightcraft: Aperture,
    materialcraft: Palette,
    pdfcraft: FileText,
    photocraft: Image,
    soundcraft: AudioLines,
    texturecraft: Grid3x3,
    vectorcraft: Spline,
    wordcraft: FileType,
  };
  const mark = (name: string) => marks[name] ?? Plug;

  // FigCraft runs inside Emberloop, so it sits apart from the apps that only bring an MCP server.
  const builtin = $derived(craft.filter((app) => app.name === 'figcraft'));
  const apps = $derived(craft.filter((app) => app.name !== 'figcraft'));
  const customTools = $derived(
    project.tools.filter((tool) => !craft.some((app) => app.name === tool.name))
  );
  const entry = (name: string) => project.tools.find((tool) => tool.name === name);

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

  function toggleCraft(name: string) {
    const tool = entry(name);
    if (tool) {
      toggle(tool);
      return;
    }
    void save([...project.tools, { name, url: '', enabled: true }]);
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
    {#snippet craftRow(app: CraftApp)}
      {@const Mark = mark(app.name)}
      <div class="group flex w-full min-w-0 items-center gap-2 rounded-[10px] px-2 py-1.5 hover:bg-surface-3">
        <span
          class="grid size-7 shrink-0 place-items-center rounded-lg border border-border bg-surface-1 text-fg-2 group-hover:text-fg-1"
        >
          <Mark class="size-4" />
        </span>
        <span class="min-w-0 flex-1">
          <span class="block truncate text-[13px] text-fg-1">{app.label}</span>
          <span class="block truncate text-[10px] text-fg-3">{app.summary}</span>
        </span>
        {#if !app.installed}
          <span class="shrink-0 rounded-full border border-border px-1.5 py-px text-[10px] text-fg-3">not installed</span>
        {/if}
        {#if app.name === 'figcraft'}
          <button
            type="button"
            class="icon-btn shrink-0 text-fg-2 hover:text-fg-1"
            aria-label="Open FigCraft"
            title="Open the design view"
            onclick={() => { onclose(); onopendesign(); }}
          >
            <ArrowUpRight class="size-4" />
          </button>
        {:else if app.web || app.url}
          <a
            class="icon-btn shrink-0 text-fg-2 hover:text-fg-1"
            href={app.web || app.url}
            target="_blank"
            rel="noopener"
            aria-label={`Open ${app.label}`}
            title={app.web ? `Open the ${app.label} editor` : `Open ${app.label} on getartcraft.com`}
          >
            <ArrowUpRight class="size-4" />
          </a>
        {/if}
        <Switch
          label={`${app.label} enabled`}
          checked={entry(app.name)?.enabled ?? false}
          disabled={busy}
          onchange={() => toggleCraft(app.name)}
        />
      </div>
    {/snippet}
    {#if builtin.length}
      <p class="px-3 pt-4 pb-1 text-[10px] font-semibold tracking-[0.08em] text-fg-3 uppercase">Built-in</p>
      {#each builtin as app (app.name)}
        {@render craftRow(app)}
      {/each}
    {/if}
    {#if apps.length}
      <p class="px-3 pt-4 pb-1 text-[10px] font-semibold tracking-[0.08em] text-fg-3 uppercase">Craft apps</p>
      {#each apps as app (app.name)}
        {@render craftRow(app)}
      {/each}
    {/if}
    {#if customTools.length}
      <p class="px-3 pt-4 pb-1 text-[10px] font-semibold tracking-[0.08em] text-fg-3 uppercase">MCP tools</p>
      {#each customTools as tool (tool.name)}
        <div class="group flex w-full min-w-0 items-center gap-2 rounded-[10px] px-2 py-1.5 hover:bg-surface-3">
          <span
            class="grid size-7 shrink-0 place-items-center rounded-lg border border-border bg-surface-1 text-fg-2 group-hover:text-fg-1"
          >
            <Plug class="size-4" />
          </span>
          <span class="min-w-0 flex-1">
            <span class="block truncate text-[13px] text-fg-1">{tool.name}</span>
            <span class="block truncate text-[10px] text-fg-3">{tool.url || 'no page'}</span>
          </span>
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
    <p class="px-3 pt-3 text-[11px] text-fg-3">Tool changes apply to new Sessions.</p>
    {#if error}<p role="alert" class="px-3 pt-1 text-[11px] text-status-failed">{error}</p>{/if}
  </nav>
</aside>
