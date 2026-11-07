<script lang="ts">
  import type { Model, Project, Session, SessionStatus } from '$lib/api/types';
  import { timeAgo } from '$lib/time';
  import { fade } from 'svelte/transition';
  import { flip, flipAnimation, flipReady, motionParams, rowEnter, slide } from '$lib/motion';
  import { marqueeShift } from '$lib/marquee';
  import { cn } from '$lib/utils';
  import Button from './ui/button.svelte';
  import * as Select from './ui/select';

  let {
    projects,
    projectId = null,
    models = [],
    sessions,
    statuses,
    selectedId,
    /** The model picked on the draft page; its harness colors the draft row. */
    draftModel = null,
    settingsActive = false,
    providersActive = false,
    usageActive = false,
    onselect,
    ondraft = () => {},
    onselectproject,
    onnew,
    onnewproject,
    onrename,
    ondelete,
    onsettings,
    onproviders,
    onusage
  }: {
    projects: Project[];
    /** The URL's Project: its Sessions are the list, and the select shows its name. */
    projectId?: string | null;
    models?: Model[];
    draftModel?: string | null;
    sessions: Session[];
    statuses: Record<string, SessionStatus>;
    selectedId: string | null;
    settingsActive?: boolean;
    providersActive?: boolean;
    usageActive?: boolean;
    onselect: (id: string) => void;
    ondraft?: () => void;
    onselectproject: (id: string) => void;
    onnew: () => void;
    onnewproject: () => void;
    onrename: (session: Session) => void;
    ondelete: (session: Session) => void;
    onsettings: () => void;
    onproviders: () => void;
    onusage: () => void;
  } = $props();

  const statusWord: Record<SessionStatus, string> = {
    idle: 'Idle',
    working: 'Working',
    failed: 'Failed',
    paused: 'Paused'
  };
  const statusOf = (id: string): SessionStatus => statuses[id] ?? 'idle';
  const ADD = '__new__';
  // A Session's harness is its model's source; a model the catalogue lost falls back to its name.
  const harnessOf = (model: string) =>
    models.find((entry) => entry.id === model)?.source ?? (model.includes('/') ? 'pi' : 'agy');
  const harnessName = (harness: string) =>
    harness === 'claude' ? 'Claude' : harness === 'pi' ? 'pi-rs' : 'Antigravity';
  const harnessColor = (harness: string) =>
    harness === 'claude' ? '#d6a87b' : harness === 'pi' ? '#8b7cf6' : '#5b9ac6';
  const projectName = $derived(projects.find((project) => project.id === projectId)?.name ?? 'Select a Project');
  const listed = $derived(
    (projectId ? sessions.filter((session) => session.projectId === projectId) : sessions)
      .sort((a, b) => b.updatedAt - a.updatedAt)
  );

  function pick(value: string) {
    if (value === ADD) onnewproject();
    else if (value !== projectId) onselectproject(value);
  }

  // A folded title slides to its tail and back while its row is hovered or selected. app.css
  // owns the animation; this measures how far the title overflows.
  function marquee(node: HTMLElement) {
    const box = node.parentElement;
    if (!box || typeof ResizeObserver === 'undefined') return;
    const measure = () => {
      const fit = marqueeShift(box.clientWidth, node.scrollWidth);
      node.classList.toggle('marquee', fit !== null);
      node.style.setProperty('--marquee-shift', `${fit?.shift ?? 0}px`);
      node.style.setProperty('--marquee-dur', `${fit?.seconds ?? 6}s`);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    return { destroy: () => observer.disconnect() };
  }
</script>

<div class="flex h-full flex-col border-r border-border bg-sidebar">
  <div class="flex flex-col gap-2 px-2 pt-3 pb-2">
    <Select.Root type="single" value={projectId ?? ''} onValueChange={pick}>
      <Select.Trigger class="w-full min-w-0 bg-surface-2 font-medium" aria-label="Project">
        {projectName}
      </Select.Trigger>
      <Select.Content>
        <Select.Item value={ADD}>+ Add project</Select.Item>
        {#each projects as project (project.id)}
          <Select.Item value={project.id}>{project.name}</Select.Item>
        {/each}
      </Select.Content>
    </Select.Root>
    <Button variant="primary" class="w-full" onclick={onnew}>New session</Button>
  </div>

  <nav class="min-h-0 flex-1 overflow-y-auto px-2 pb-2" aria-label="Sessions">
    {#if draftModel}
      <div class="row sel flex items-center gap-1 px-1">
        <button class="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left" aria-current="true" onclick={ondraft}>
          <span
            class="h-5 w-[3px] shrink-0 rounded-full"
            style:background={harnessColor(harnessOf(draftModel))}
            title={`${harnessName(harnessOf(draftModel))} · Draft`}
          ></span>
          <span class="min-w-0 flex-1 truncate text-[13px]">New session</span>
        </button>
      </div>
    {/if}
    {#each listed as session (session.id)}
      {@const status = statusOf(session.id)}
      {@const harness = harnessOf(session.model)}
      <div
        class={cn('row group flex items-center gap-1 px-1', selectedId === session.id && 'sel')}
        use:flipReady
        animate:flip={flipAnimation()}
        in:slide={rowEnter}
        out:fade={motionParams()}
      >
        <button
          class="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left"
          aria-current={selectedId === session.id ? 'true' : undefined}
          onclick={() => onselect(session.id)}
        >
          <span
            class={cn('h-5 w-[3px] shrink-0 rounded-full', status === 'working' && 'pulse')}
            style:background={harnessColor(harness)}
            title={`${harnessName(harness)} · ${statusWord[status]}`}
          ></span>
          <span class="min-w-0 flex-1 overflow-hidden whitespace-nowrap">
            <span class="inline-block text-[13px]" use:marquee>{session.title}</span>
          </span>
          <span class="text-[11px] text-fg-2">{timeAgo(session.updatedAt)}</span>
        </button>
        <button
          class="hidden rounded px-1 text-[12px] text-fg-3 hover:text-fg-1 group-hover:block focus:block max-md:block"
          title="Rename session"
          aria-label={`Rename ${session.title}`}
          onclick={() => onrename(session)}
        >✎</button>
        <button
          class="hidden rounded px-1 text-[12px] text-fg-3 hover:text-status-failed group-hover:block focus:block max-md:block"
          title="Delete session"
          aria-label={`Delete ${session.title}`}
          onclick={() => ondelete(session)}
        >×</button>
      </div>
    {/each}
    {#if projects.length === 0}
      <p class="px-3 py-2 text-[13px] text-fg-3">No Projects yet. Add one from the Project select.</p>
    {/if}
  </nav>

  <div class="border-t border-border p-2">
    <button
      class={cn('row w-full px-3 py-2 text-left text-[14px]', usageActive && 'sel')}
      onclick={onusage}
    >Usage</button>
    <button
      class={cn('row w-full px-3 py-2 text-left text-[14px]', providersActive && 'sel')}
      onclick={onproviders}
    >Providers</button>
    <button
      class={cn('row w-full px-3 py-2 text-left text-[14px]', settingsActive && 'sel')}
      onclick={onsettings}
    >Settings</button>
  </div>
</div>
