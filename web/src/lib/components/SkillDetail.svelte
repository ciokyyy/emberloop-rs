<script lang="ts">
  import { api } from '$lib/api';
  import { lineDiff } from '$lib/skilldiff';
  import type { SkillDetail as Detail, SkillHistory } from '$lib/api/types';
  import Icon from './Icon.svelte';

  let { name, path = $bindable(''), onback }: {
    name: string;
    /** The shell's header subtitle shows the SKILL.md path, which only the loaded Skill knows. */
    path?: string;
    onback: () => void;
  } = $props();

  let skill = $state<Detail | null>(null);
  let history = $state<SkillHistory | null>(null);
  let error = $state('');

  $effect(() => {
    error = '';
    skill = null;
    history = null;
    void (async () => {
      try {
        [skill, history] = await Promise.all([api.skillDetail(name), api.skillHistory(name)]);
        path = skill.path;
      } catch (cause) {
        error = String(cause);
      }
    })();
  });

  const when = (secs: number) => new Date(secs * 1000).toLocaleString();

  type Entry = { at: number; file: string; lines: { kind: 'same' | 'add' | 'del'; text: string }[] | null };

  // A backup holds the SKILL.md text before one change. It diffs against the text that followed:
  // the newest backup against the current file, each older one against the backup just after it.
  function entries(list: SkillHistory, current: Detail): Entry[] {
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

<section class="flex min-h-0 flex-1 flex-col gap-4 overflow-auto bg-surface-0 p-4">
  {#if error}
    <p class="text-[13px] text-red-400">{error}</p>
  {/if}

  <div class="flex h-9 items-center justify-between gap-3">
    <div class="flex items-center gap-2">
      <button class="grid size-8 place-items-center rounded-lg border border-border text-fg-2 hover:bg-surface-1 hover:text-fg-1"
        aria-label="All skills" onclick={onback}>
        <Icon name="chevron-left" class="size-4" />
      </button>
      <span class="text-[13px] text-fg-3">All skills</span>
    </div>
    {#if skill}
      <span class="rounded-full border border-border px-2 py-0.5 text-[11px] text-fg-3">{skill.origin}</span>
    {/if}
  </div>

  {#if skill}
    <pre class="whitespace-pre-wrap rounded-lg border border-border bg-sidebar p-3 font-mono text-[12px] text-fg-2">{skill.text}</pre>

    <div class="grid gap-0.5">
      <p class="px-2 pt-1.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-fg-3">Files ({skill.files.length})</p>
      {#each skill.files as file (file.path)}
        <details class="row px-3">
          <summary class="flex h-[34px] cursor-pointer items-center gap-2 text-[12px] text-fg-2">
            <Icon name="file-text" class="size-4 shrink-0 text-fg-3" />
            <span class="min-w-0 flex-1 truncate">{file.path}</span>
            <Icon name="chevron-down" class="size-4 shrink-0 text-fg-4" />
          </summary>
          <pre class="whitespace-pre-wrap rounded-lg border border-border bg-sidebar p-3 pb-2 font-mono text-[12px] text-fg-2">{file.text ?? 'Not text, or too large to show.'}</pre>
        </details>
      {:else}
        <p class="px-3 py-2 text-[13px] text-fg-3">No supporting files.</p>
      {/each}
    </div>

    <div class="grid gap-2">
      <p class="px-2 pt-1.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-fg-3">History ({history?.backups.length ?? 0})</p>
      {#if history}
        {#each entries(history, skill) as entry (entry.at + entry.file)}
          <div class="grid gap-1.5">
            <p class="text-[12px] text-fg-3">{when(entry.at)} · {entry.file}</p>
            {#if entry.lines === null}
              <p class="text-[12px] text-fg-3">Supporting file changed. Text not kept.</p>
            {:else if entry.lines.length === 0}
              <p class="text-[12px] text-fg-3">No text change.</p>
            {:else}
              <div class="rounded-lg border border-border bg-sidebar py-1.5 font-mono text-[12px]">
                {#each entry.lines as line}
                  <span class={line.kind === 'add' ? 'diff-add block px-2.5 py-0.5' : 'diff-del block px-2.5 py-0.5'}>{line.kind === 'add' ? '+' : '-'} {line.text}</span>
                {/each}
              </div>
            {/if}
          </div>
        {:else}
          <p class="text-[13px] text-fg-3">No changes recorded.</p>
        {/each}
        {#if history.usage.length}
          <details>
            <summary class="cursor-pointer text-[12px] text-fg-3">Usage log ({history.usage.length})</summary>
            <ul class="grid gap-0.5 py-1 font-mono text-[12px] text-fg-3">
              {#each history.usage as line}
                <li>{when(line.time)} · {line.op} · {line.result}</li>
              {/each}
            </ul>
          </details>
        {/if}
      {/if}
    </div>
  {:else if !error}
    <p class="text-[13px] text-fg-3">Loading…</p>
  {/if}
</section>
