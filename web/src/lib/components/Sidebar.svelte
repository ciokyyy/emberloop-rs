<script lang="ts">
  import type { Model, Project, Session, SessionStatus } from '$lib/api/types';
  import { timeAgo } from '$lib/time';
  import { fade } from 'svelte/transition';
  import { flip, flipAnimation, flipReady, motionParams, rowEnter, slide } from '$lib/motion';
  import { cn } from '$lib/utils';
  import Button from './ui/button.svelte';

  let {
    projects,
    models = [],
    sessions,
    statuses,
    selectedId,
    settingsActive = false,
    providersActive = false,
    usageActive = false,
    onselect,
    onnew,
    onnewproject,
    onrename,
    ondelete,
    onsettings,
    onproviders,
    onusage
  }: {
    projects: Project[];
    models?: Model[];
    sessions: Session[];
    statuses: Record<string, SessionStatus>;
    selectedId: string | null;
    settingsActive?: boolean;
    providersActive?: boolean;
    usageActive?: boolean;
    onselect: (id: string) => void;
    onnew: () => void;
    /** When set, the second button opens the new-Project dialog. */
    onnewproject?: () => void;
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
</script>

<div class="flex h-full flex-col border-r border-border bg-sidebar">
  <div class="flex items-center justify-between px-4 pt-4 pb-3">
    <div class="font-semibold tracking-tight">Emberloop</div>
  </div>
  <div class="flex gap-2 px-3 pb-2">
    <Button variant="primary" class="flex-1" onclick={onnew}>New session</Button>
    {#if onnewproject}
      <Button class="flex-1" onclick={onnewproject}>New project</Button>
    {/if}
  </div>

  <nav class="min-h-0 flex-1 overflow-y-auto px-2 pb-2" aria-label="Projects and sessions">
    {#each projects as project (project.id)}
      {@const list = sessions
        .filter((session) => session.projectId === project.id)
        .sort((a, b) => b.updatedAt - a.updatedAt)}
      <div class="mb-2">
        <div class="flex justify-between px-3 py-1 text-[12px] tracking-[0.04em] text-fg-3">
          <span class="uppercase">{project.name}</span>
          <span class="font-mono" title={project.root}>{list.length}</span>
        </div>
        {#each list as session (session.id)}
          {@const status = statusOf(session.id)}
          {@const harness = models.find((model) => model.id === session.model)?.source ?? (session.model.includes('/') ? 'pi' : 'agy')}
          <div
            class={cn('row group flex items-center gap-1 px-1', selectedId === session.id && 'sel')}
            use:flipReady
            animate:flip={flipAnimation()}
            in:slide={rowEnter}
            out:fade={motionParams()}
          >
            <button
              class="flex min-w-0 flex-1 items-center gap-2.5 px-2 py-2 text-left"
              aria-current={selectedId === session.id ? 'true' : undefined}
              onclick={() => onselect(session.id)}
            >
              <span
                class={cn('h-7 w-[3px] shrink-0 rounded-full', status === 'working' && 'pulse')}
                style:background={harness === 'claude' ? '#d6a87b' : harness === 'pi' ? '#ce614a' : '#5b9ac6'}
                title={`${harness === 'claude' ? 'Claude' : harness === 'pi' ? 'pi-rs' : 'Antigravity'} · ${statusWord[status]}`}
              ></span>
              <span class="min-w-0 flex-1">
                <span class="block truncate text-[14px]">{session.title}</span>
              </span>
              <span class="text-[12px] text-fg-2">{timeAgo(session.updatedAt)}</span>
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
      </div>
    {/each}
    {#if projects.length === 0}
      <p class="px-3 py-2 text-[13px] text-fg-3">No Projects yet. Create one from New session.</p>
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
