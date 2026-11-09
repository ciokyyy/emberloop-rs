<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { lineDiff } from '$lib/skilldiff';
  import type { SkillDetail, SkillHistory, SkillSummary } from '$lib/api/types';

  let skills = $state<SkillSummary[]>([]);
  let selected = $state<SkillDetail | null>(null);
  let history = $state<SkillHistory | null>(null);
  let error = $state('');

  onMount(async () => {
    try {
      skills = await api.listSkills();
    } catch (cause) {
      error = String(cause);
    }
  });

  async function open(name: string) {
    error = '';
    try {
      [selected, history] = await Promise.all([api.skillDetail(name), api.skillHistory(name)]);
    } catch (cause) {
      error = String(cause);
    }
  }

  const when = (secs: number) => new Date(secs * 1000).toLocaleString();

  type Entry = { at: number; file: string; lines: { kind: 'same' | 'add' | 'del'; text: string }[] | null };

  // A backup holds the SKILL.md text before one change. It diffs against the text that followed:
  // the newest backup against the current file, each older one against the backup just after it.
  function entries(list: SkillHistory, current: SkillDetail): Entry[] {
    const main = list.backups.filter((backup) => backup.file === 'SKILL.md');
    let index = 0;
    return list.backups.map((backup) => {
      if (backup.file !== 'SKILL.md') return { at: backup.at, file: backup.file, lines: null };
      const after = index === 0 ? current.text : main[index - 1].text;
      index += 1;
      return { at: backup.at, file: backup.file, lines: lineDiff(backup.text, after).filter((line) => line.kind !== 'same') };
    });
  }
</script>

<section class="flex min-h-0 flex-1 flex-col gap-4 overflow-auto p-4">
  {#if error}
    <p class="text-[13px] text-red-400">{error}</p>
  {/if}

  <div class="grid gap-1">
    {#each skills as skill (skill.path)}
      <button class="row flex flex-col items-start gap-0.5 px-3 py-2 text-left" onclick={() => open(skill.name)}>
        <span class="font-medium">{skill.name}</span>
        <span class="text-[12px] text-fg-3">{skill.project ? 'project' : 'global'} · {skill.description}</span>
      </button>
    {:else}
      <p class="px-3 py-2 text-[13px] text-fg-3">No agent-managed skills yet.</p>
    {/each}
  </div>

  {#if selected}
    <article class="grid gap-3">
      <div>
        <h2 class="font-semibold">{selected.name}</h2>
        <p class="text-[12px] text-fg-3">{selected.path}</p>
      </div>
      <pre class="whitespace-pre-wrap rounded border border-border p-3 text-[12px]">{selected.text}</pre>

      {#each selected.files as file (file.path)}
        <details>
          <summary class="cursor-pointer text-[13px]">{file.path}</summary>
          <pre class="whitespace-pre-wrap rounded border border-border p-3 text-[12px]">{file.text ?? 'Not text, or too large to show.'}</pre>
        </details>
      {/each}

      <h3 class="font-semibold">History</h3>
      {#if history}
        {#each entries(history, selected) as entry (entry.at + entry.file)}
          <div class="grid gap-1">
            <p class="text-[12px] text-fg-3">{when(entry.at)} · {entry.file}</p>
            {#if entry.lines === null}
              <p class="text-[12px] text-fg-3">Supporting file changed. Text not kept.</p>
            {:else if entry.lines.length === 0}
              <p class="text-[12px] text-fg-3">No text change.</p>
            {:else}
              <pre class="whitespace-pre-wrap rounded border border-border p-3 text-[12px]">{#each entry.lines as line}<span class={line.kind === 'add' ? 'text-green-400' : 'text-red-400'}>{line.kind === 'add' ? '+' : '-'} {line.text}
</span>{/each}</pre>
            {/if}
          </div>
        {:else}
          <p class="text-[13px] text-fg-3">No changes recorded.</p>
        {/each}
        {#if history.usage.length}
          <details>
            <summary class="cursor-pointer text-[13px]">Usage log ({history.usage.length})</summary>
            <ul class="grid gap-0.5 text-[12px] text-fg-3">
              {#each history.usage as line}
                <li>{when(line.time)} · {line.op} · {line.result}</li>
              {/each}
            </ul>
          </details>
        {/if}
      {/if}
    </article>
  {/if}
</section>
