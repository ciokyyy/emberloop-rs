<script lang="ts">
  import { api } from '$lib/api';
  import type { SkillSummary } from '$lib/api/types';
  import Icon from './Icon.svelte';

  let { projectId = null, onopen = () => {} }: {
    projectId?: string | null;
    /** One Skill's page is a route, so the shell owns the navigation. */
    onopen?: (name: string) => void;
  } = $props();

  let skills = $state<SkillSummary[]>([]);
  let error = $state('');

  const scope = $derived(skills.filter((skill) => skill.project));
  const shared = $derived(skills.filter((skill) => !skill.project));

  // One call covers both scopes: the project root first, then the global root, whatever the origin.
  $effect(() => {
    void (async () => {
      try {
        skills = await api.listSkills(projectId);
      } catch (cause) {
        error = String(cause);
      }
    })();
  });
</script>

<section class="flex min-h-0 flex-1 flex-col gap-2 overflow-auto bg-surface-0 p-4">
  {#if error}
    <p class="text-[13px] text-red-400">{error}</p>
  {/if}

  {#each [{ label: 'This project', list: scope }, { label: 'Global', list: shared }] as group (group.label)}
    <div class="grid gap-0.5">
      <p class="px-2 pt-1.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-fg-3">
        {group.label} ({group.list.length})
      </p>
      {#each group.list as skill (skill.path)}
        <button class="row flex h-[60px] items-center gap-3 px-3 py-2.5 text-left" onclick={() => onopen(skill.name)}>
          <span class="grid min-w-0 flex-1 gap-0.5">
            <span class="truncate text-[13px] font-semibold text-fg-1">{skill.name}</span>
            <span class="truncate text-[12px] text-fg-3">{skill.description}</span>
          </span>
          <span class="shrink-0 rounded-full border border-border px-2 py-0.5 text-[11px] text-fg-3">{skill.origin}</span>
          <Icon name="chevron-right" class="size-4 shrink-0 text-fg-4" />
        </button>
      {:else}
        <p class="px-3 py-2 text-[13px] text-fg-3">No skills here.</p>
      {/each}
    </div>
  {/each}
</section>
