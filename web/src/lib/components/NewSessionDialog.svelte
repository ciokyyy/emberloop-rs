<script lang="ts">
  import { Dialog } from 'bits-ui';
  import { fadeAt, rise } from '$lib/motion';
  import type { Model, Project } from '$lib/api/types';
  import Button from './ui/button.svelte';
  import ModelPicker from './ModelPicker.svelte';

  let {
    open = $bindable(false),
    projects,
    // Asking for a new Project by path only: no Project list.
    newProject = false,
    models = [],
    defaultLevel = 'full',
    oncreate
  }: {
    open?: boolean;
    projects: Project[];
    newProject?: boolean;
    models?: Model[];
    defaultLevel?: string;
    oncreate: (input: {
      projectId?: string;
      root?: string;
      ponytailLevel: string;
      model?: string;
      thoughtLevel?: string;
    }) => Promise<void> | void;
  } = $props();

  let projectId = $state('');
  let root = $state('');
  let level = $state('full');
  let model = $state('');
  let effort = $state('');
  const efforts = $derived(models.find((entry) => entry.id === model)?.efforts ?? []);
  $effect(() => { if (!efforts.includes(effort)) effort = efforts[0] ?? ''; });
  let error = $state('');
  let busy = $state(false);

  const levels = ['off', 'lite', 'full', 'ultra'];

  $effect(() => {
    if (open) {
      projectId = projects[0]?.id ?? '';
      root = '';
      level = defaultLevel;
      model = models[0]?.id ?? '';
      error = '';
    }
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const targetProject = root.trim() ? undefined : projectId;
    if (newProject && !root.trim()) {
      error = 'Enter the path of the Project to add.';
      return;
    }
    if (!targetProject && !root.trim()) {
      error = 'Choose a Project or enter a path.';
      return;
    }
    busy = true;
    error = '';
    try {
      await oncreate({
        projectId: targetProject,
        root: root.trim() || undefined,
        ponytailLevel: level,
        model: model || undefined,
        thoughtLevel: effort || undefined
      });
      open = false;
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Portal>
    <Dialog.Overlay forceMount class="fixed inset-0 z-40 bg-black/60">
      {#snippet child({ props, open: shown })}
        {#if shown}<div {...props} transition:fadeAt></div>{/if}
      {/snippet}
    </Dialog.Overlay>
    <Dialog.Content
      forceMount
      class="fixed left-1/2 top-1/2 z-50 flex max-h-[calc(100dvh-2rem)] w-[min(92vw,440px)] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 overflow-y-auto rounded-2xl border border-border bg-surface-2 p-5"
    >
      {#snippet child({ props, open: shown })}
        {#if shown}
          <div {...props} transition:rise>
      <Dialog.Title class="text-[1.0625rem] font-semibold">
        {newProject ? 'New project' : 'New session'}
      </Dialog.Title>
      <Dialog.Description class="sr-only">
        Create a Session in an existing Project or in a new Project by path.
      </Dialog.Description>

      <form class="flex flex-col gap-4" onsubmit={submit}>
        {#if projects.length && !newProject}
          <label class="flex flex-col gap-1.5 text-[13px] text-fg-3">
            Project
            <select class="field" bind:value={projectId} disabled={!!root.trim()}>
              {#each projects as project (project.id)}
                <option value={project.id}>{project.name} — {project.root}</option>
              {/each}
            </select>
          </label>
        {/if}

        <label class="flex flex-col gap-1.5 text-[13px] text-fg-3">
          {newProject ? 'Project path' : 'Or add a Project by path'}
          <input class="field font-mono" bind:value={root} placeholder="/home/you/Projects/name" />
        </label>

        {#if !newProject}
        <ModelPicker searchable {models} bind:value={model} />
        {#if efforts.length}
          <label class="flex flex-col gap-1.5 text-[12px] text-fg-3">
            Reasoning effort
            <select class="field" bind:value={effort}>
              {#each efforts as option (option)}<option value={option}>{option}</option>{/each}
            </select>
          </label>
        {/if}

        <div class="flex flex-col gap-1.5 text-[13px] text-fg-3">
          Ponytail level
          <div class="seg self-start">
            {#each levels as option (option)}
              <button type="button" aria-pressed={level === option} onclick={() => (level = option)}>
                {option}
              </button>
            {/each}
          </div>
        </div>

        {/if}

        {#if error}<p class="text-[12px] text-status-failed">{error}</p>{/if}

        <div class="flex justify-end gap-2">
          <Button onclick={() => (open = false)}>Cancel</Button>
          <Button variant="primary" type="submit" disabled={busy}>Create</Button>
        </div>
      </form>
          </div>
        {/if}
      {/snippet}
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
