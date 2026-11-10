<script lang="ts">
  import type { Model, Project, Session, SessionStatus } from '$lib/api/types';
  import { fade } from 'svelte/transition';
  import { flip, flipAnimation, flipReady, motionParams, rowEnter, slide } from '$lib/motion';
  import { marqueeShift } from '$lib/marquee';
  import { cn } from '$lib/utils';
  import Button from './ui/button.svelte';
  import Select from './Select.svelte';
  import { holdToLift } from '$lib/hold';
  import * as AlertDialog from './ui/alert-dialog';

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
    skillsActive = false,
    onselect,
    ondraft = () => {},
    onselectproject,
    onnew,
    onnewproject,
    onrename,
    ondelete,
    onsettings,
    onproviders,
    onusage,
    onskills = () => {}
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
    skillsActive?: boolean;
    onselect: (id: string) => void;
    ondraft?: () => void;
    onselectproject: (id: string) => void;
    onnew: () => void;
    onnewproject: () => void;
    onrename: (session: Session, title: string) => void;
    ondelete: (session: Session) => void;
    onsettings: () => void;
    onproviders: () => void;
    onusage: () => void;
    onskills?: () => void;
  } = $props();

  const statusWord: Record<SessionStatus, string> = {
    idle: 'Idle',
    working: 'Working',
    failed: 'Failed',
    paused: 'Paused'
  };
  const statusOf = (id: string): SessionStatus => statuses[id] ?? 'idle';
  const ADD = '__new__';
  // The switcher's second line: the Project's root, so two Projects of the same name stay apart.
  const rootOf = (id: string | null) => projects.find((project) => project.id === id)?.root ?? '';
  // A Session's harness is its model's source; a model the catalogue lost falls back to its name.
  const harnessOf = (model: string) =>
    models.find((entry) => entry.id === model)?.source ?? (model.includes('/') ? 'pi' : 'agy');
  const harnessName = (harness: string) =>
    harness === 'claude' ? 'Claude' : harness === 'pi' ? 'pi-rs' : 'Antigravity';
  const harnessColor = (harness: string) =>
    harness === 'claude' ? '#d6a87b' : harness === 'pi' ? '#8b7cf6' : '#5b9ac6';
  type ProjectGroup = {
    project: Project;
    hasWorking: boolean;
    sessions: Session[];
  };

  const projectGroups = $derived.by(() => {
    const groups: ProjectGroup[] = [];
    for (const project of projects) {
      const projSessions = sessions
        .filter((session) => session.projectId === project.id)
        .sort((a, b) => {
          const aWorking = statusOf(a.id) === 'working';
          const bWorking = statusOf(b.id) === 'working';
          if (aWorking !== bWorking) return aWorking ? -1 : 1;
          return b.updatedAt - a.updatedAt;
        });
      if (projSessions.length > 0) {
        groups.push({
          project,
          hasWorking: projSessions.some((session) => statusOf(session.id) === 'working'),
          sessions: projSessions
            .filter((session) => !session.parentId || !projSessions.some((parent) => parent.id === session.parentId))
            .flatMap((parent) => [parent, ...projSessions.filter((child) => child.parentId === parent.id)])
        });
      }
    }
    return groups.sort((a, b) => {
      if (a.hasWorking !== b.hasWorking) return a.hasWorking ? -1 : 1;
      if (projectId && a.project.id === projectId) return -1;
      if (projectId && b.project.id === projectId) return 1;
      return a.project.name.localeCompare(b.project.name);
    });
  });

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

  // Double-tap edits a title in place. Enter and blur both commit; the first to run clears `editing`, so the second is a no-op.
  let editing = $state<string | null>(null);
  let draft = $state('');
  let holding = $state<string | null>(null);
  let dragging = $state<Session | null>(null);
  let overTrash = $state(false);
  let pending = $state<Session | null>(null);
  let trashEl: HTMLElement | undefined = $state();
  const HOLD_MS = 450;

  function focusAll(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
  function startRename(session: Session) {
    editing = session.id;
    draft = session.title;
  }
  function commitRename(session: Session) {
    if (editing !== session.id) return;
    editing = null;
    const title = draft.trim();
    if (title && title !== session.title) onrename(session, title);
  }
  function cancelRename() {
    editing = null;
  }

  const inside = (el: HTMLElement | undefined, event: PointerEvent) => {
    if (!el) return false;
    const box = el.getBoundingClientRect();
    return event.clientX >= box.left && event.clientX <= box.right && event.clientY >= box.top && event.clientY <= box.bottom;
  };
  // Hold 450 ms to lift a row; drag it onto the trash zone and release to ask for delete.
  function holdStart(event: PointerEvent, session: Session) {
    if (event.button !== 0 || editing === session.id) return;
    holding = session.id;
    holdToLift(event, HOLD_MS, {
      lift: () => {
        holding = null;
        dragging = session;
      },
      move: (move) => {
        overTrash = inside(trashEl, move);
      },
      drop: (up, lifted) => {
        holding = null;
        if (lifted && inside(trashEl, up)) pending = session;
        dragging = null;
        overTrash = false;
      }
    });
  }
  // Keyboard path to the same confirm dialog as the trash drag.
  function onRowKey(event: KeyboardEvent, session: Session) {
    if (event.key !== 'Delete') return;
    event.preventDefault();
    pending = session;
  }
  function confirmDelete() {
    if (pending) ondelete(pending);
    pending = null;
  }
</script>

<div class="flex h-full flex-col border-r border-border bg-sidebar">
  <div class="flex items-center gap-2 px-2 py-2.5">
    <Select label="Project" placeholder="Select a Project"
      class="h-[45px] min-w-0 flex-1 rounded-lg border-transparent bg-transparent px-2 py-1 hover:bg-surface-2"
      value={projectId ?? ''} onchange={pick}
      options={[{ value: ADD, label: '+ Add project' }, ...projects.map((project) => ({ value: project.id, label: project.name }))]}>
      {#snippet trigger(name: string)}
        <svg viewBox="0 0 24 24" class="h-4 w-4 shrink-0 fill-none stroke-current stroke-[1.75] text-fg-2" aria-hidden="true"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" /></svg>
        <span class="grid min-w-0 flex-1">
          <span class="truncate text-[13px] text-fg-1">{name}</span>
          <span class="truncate text-[11px] text-fg-3">{rootOf(projectId)}</span>
        </span>
        <svg viewBox="0 0 24 24" class="h-4 w-4 shrink-0 fill-none stroke-current stroke-[1.75] text-fg-3" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
      {/snippet}
    </Select>
    <button class="grid h-8 w-8 shrink-0 place-items-center rounded-xl border border-border bg-surface-1 text-fg-2 hover:bg-surface-3 hover:text-fg-1"
      aria-label="New session" title="New session" onclick={onnew}>
      <svg viewBox="0 0 24 24" class="h-4 w-4 fill-none stroke-current stroke-[1.75]" aria-hidden="true"><path d="M5 12h14" /><path d="M12 5v14" /></svg>
    </button>
  </div>

  <nav class="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pb-2" aria-label="Sessions">
    {#if dragging}
      <div
        bind:this={trashEl}
        class={cn(
          'mb-2 flex items-center justify-center gap-2 rounded-lg border border-dashed border-border py-3 text-[13px] text-fg-2',
          overTrash && 'border-status-failed text-status-failed'
        )}
        aria-hidden="true"
      >
        <svg viewBox="0 0 24 24" class="h-4 w-4 fill-none stroke-current stroke-[1.75]"><path d="M3 6h18" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" /><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /></svg>
        Drop to delete
      </div>
    {/if}
    {#if draftModel}
      <div class="row sel flex h-8 items-center px-2">
        <button class="flex h-full min-w-0 flex-1 items-center gap-2 text-left" aria-current="true" onclick={ondraft}>
          <span
            class="h-5 w-[3px] shrink-0 rounded-full"
            style:background={harnessColor(harnessOf(draftModel))}
            title={`${harnessName(harnessOf(draftModel))} · Draft`}
          ></span>
          <span class="min-w-0 flex-1 truncate text-[13px]">New session</span>
        </button>
      </div>
    {/if}
    {#each projectGroups as group (group.project.id)}
      <div data-testid="project-header" class="px-2 pt-2 pb-1 text-[10px] font-semibold uppercase tracking-wider text-fg-3">
        {group.project.name}
      </div>
      {#each group.sessions as session (session.id)}
        {@const status = statusOf(session.id)}
        {@const harness = harnessOf(session.model)}
        <div
          class={cn(
            'row group flex h-8 items-center px-2',
            session.parentId && 'ml-5 border-l border-border',
            selectedId === session.id && 'sel',
            holding === session.id && 'holding',
            dragging?.id === session.id && 'lifted'
          )}
          use:flipReady
          animate:flip={flipAnimation()}
          in:slide={rowEnter}
          out:fade={motionParams()}
        >
          {#if editing === session.id}
            <span class="h-5 w-[3px] shrink-0 rounded-full" style:background={harnessColor(harness)}></span>
            <input
              class="min-w-0 flex-1 bg-transparent text-[13px] outline-none"
              aria-label="Session title"
              bind:value={draft}
              use:focusAll
              onkeydown={(event) => {
                if (event.key === 'Enter') commitRename(session);
                else if (event.key === 'Escape') cancelRename();
              }}
              onblur={() => commitRename(session)}
            />
          {:else}
            <button
              class="flex h-full min-w-0 flex-1 items-center gap-2 text-left"
              aria-current={selectedId === session.id ? 'true' : undefined}
              onpointerdown={(event) => holdStart(event, session)}
              onclick={(event) => {
                onselect(session.id);
                // A double-tap (or double-click) edits the title in place.
                if (event.detail === 2) startRename(session);
              }}
              onkeydown={(event) => onRowKey(event, session)}
            >
              <span
                class={cn('h-5 w-[3px] shrink-0 rounded-full', status === 'working' && 'pulse')}
                style:background={harnessColor(harness)}
                title={`${harnessName(harness)} · ${statusWord[status]}`}
              ></span>
              <span class="min-w-0 flex-1 overflow-hidden whitespace-nowrap">
                <span class="inline-block text-[13px]" use:marquee>{session.title}</span>
              </span>
              {#if status === 'working'}
                <span class="running-dot h-2 w-2 rounded-full bg-[var(--status-working)] shrink-0" aria-label="Working"></span>
              {/if}
            </button>
          {/if}
        </div>
      {/each}
    {/each}
    {#if projects.length === 0}
      <p class="px-3 py-2 text-[13px] text-fg-3">No Projects yet. Add one from the Project select.</p>
    {/if}
  </nav>

  <div class="grid grid-cols-4 gap-1 border-t border-border p-2">
    <button class={cn('row flex flex-col items-center gap-1 py-1.5 text-[11px]', usageActive ? 'sel text-fg-1' : 'text-fg-3 hover:text-fg-1')} aria-label="Usage" onclick={onusage}>
      <svg viewBox="0 0 24 24" class="h-4 w-4 fill-none stroke-current stroke-[1.75]" aria-hidden="true"><path d="M3 3v16a2 2 0 0 0 2 2h16" /><path d="M18 17V9" /><path d="M13 17V5" /><path d="M8 17v-3" /></svg>
      Usage
    </button>
    <button class={cn('row flex flex-col items-center gap-1 py-1.5 text-[11px]', skillsActive ? 'sel text-fg-1' : 'text-fg-3 hover:text-fg-1')} aria-label="Skills" onclick={onskills}>
      <svg viewBox="0 0 24 24" class="h-4 w-4 fill-none stroke-current stroke-[1.75]" aria-hidden="true"><path d="M4 19.5V5a2 2 0 0 1 2-2h12v16H6a2 2 0 0 0-2 2Z" /><path d="M8 7h6" /><path d="M8 11h4" /></svg>
      Skills
    </button>
    <button class={cn('row flex flex-col items-center gap-1 py-1.5 text-[11px]', providersActive ? 'sel text-fg-1' : 'text-fg-3 hover:text-fg-1')} aria-label="Providers" onclick={onproviders}>
      <svg viewBox="0 0 24 24" class="h-4 w-4 fill-none stroke-current stroke-[1.75]" aria-hidden="true"><path d="M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4Z" /><path d="m2 22 3-3" /><path d="M7.5 13.5 10 11" /><path d="M10.5 16.5 13 14" /><path d="m18 3-4 4h6l-4 4" /></svg>
      Providers
    </button>
    <button class={cn('row flex flex-col items-center gap-1 py-1.5 text-[11px]', settingsActive ? 'sel text-fg-1' : 'text-fg-3 hover:text-fg-1')} aria-label="Settings" onclick={onsettings}>
      <svg viewBox="0 0 24 24" class="h-4 w-4 fill-none stroke-current stroke-[1.75]" aria-hidden="true"><line x1="21" x2="14" y1="4" y2="4" /><line x1="10" x2="3" y1="4" y2="4" /><line x1="21" x2="12" y1="12" y2="12" /><line x1="8" x2="3" y1="12" y2="12" /><line x1="21" x2="16" y1="20" y2="20" /><line x1="12" x2="3" y1="20" y2="20" /><line x1="14" x2="14" y1="2" y2="6" /><line x1="8" x2="8" y1="10" y2="14" /><line x1="16" x2="16" y1="18" y2="22" /></svg>
      Settings
    </button>
  </div>
</div>

<AlertDialog.Root open={pending !== null} onOpenChange={(open) => { if (!open) pending = null; }}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete session?</AlertDialog.Title>
      <AlertDialog.Description>“{pending?.title}” will be deleted. This cannot be undone.</AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action onclick={confirmDelete}>Delete</AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  /* Hold fill: grows across the row while the pointer is held down. */
  .holding {
    background: linear-gradient(to right, rgb(255 255 255 / 0.1), rgb(255 255 255 / 0.1)) no-repeat;
    background-size: 0% 100%;
    animation: hold 450ms linear forwards;
  }
  .lifted {
    opacity: 0.6;
  }
  @keyframes hold {
    to {
      background-size: 100% 100%;
    }
  }
</style>
