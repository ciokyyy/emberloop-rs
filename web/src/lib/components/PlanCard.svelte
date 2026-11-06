<script lang="ts">
  import type { TodoItem, TodoStatus } from '$lib/feed';

  let {
    items,
    autoExpand = true,
    hideWhenDone = true
  }: {
    items: TodoItem[];
    autoExpand?: boolean;
    hideWhenDone?: boolean;
  } = $props();

  const MARKS: Record<TodoStatus, string> = {
    pending: '[ ]',
    in_progress: '[~]',
    completed: '[x]',
    cancelled: '[–]'
  };
  const ROWS = 8;
  const AUTO_EXPAND_MS = 2000;
  const FADE_MS = 4000;

  const open = $derived(
    items.filter((item) => item.status === 'pending' || item.status === 'in_progress')
  );
  const done = $derived(items.length - open.length);
  const active = $derived(open.find((item) => item.status === 'in_progress') ?? open[0] ?? null);
  const percent = $derived(items.length ? Math.round((done / items.length) * 100) : 0);
  const visible = $derived(items.slice(0, ROWS));
  // The active item stays in view even when it sits past the cap.
  const pinned = $derived(active && !visible.includes(active) ? active : null);
  const signature = $derived(items.map((item) => `${item.id}:${item.status}:${item.content}`).join('|'));

  let expanded = $state(false);
  let hidden = $state(false);
  let previous = '';

  // A create or a replace opens the card for a moment; the same list re-rendered
  // does not.
  $effect(() => {
    const next = signature;
    if (next === previous) return;
    previous = next;
    hidden = false;
    if (!autoExpand || open.length === 0) return;
    expanded = true;
    const timer = setTimeout(() => (expanded = false), AUTO_EXPAND_MS);
    return () => clearTimeout(timer);
  });

  // No open item hides the card; `hideWhenDone` keeps it visible for a moment first.
  $effect(() => {
    if (items.length === 0 || open.length > 0) return;
    if (!hideWhenDone) {
      hidden = true;
      return;
    }
    const timer = setTimeout(() => (hidden = true), FADE_MS);
    return () => clearTimeout(timer);
  });

  $effect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'p' || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)
      ) {
        return;
      }
      if (items.length === 0) return;
      expanded = !expanded;
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });
</script>

{#if items.length > 0 && !hidden}
  <section
    class="rounded-2xl border border-border bg-surface-1"
    data-testid="plan-card"
  >
    <button
      class="flex w-full items-center gap-2 px-3 py-2 text-left text-[13px]"
      aria-expanded={expanded}
      data-testid="plan-toggle"
      onclick={() => (expanded = !expanded)}
    >
      <span class="chip shrink-0">{done}/{items.length}</span>
      <span class="min-w-0 flex-1 truncate text-fg-2" data-testid="plan-active">
        {active ? `${active.id} ${active.content}` : 'All items resolved'}
      </span>
      <span class="shrink-0 text-fg-3" aria-hidden="true">{expanded ? '▾' : '▸'}</span>
    </button>
    <div class="mx-3 mb-2 h-1 overflow-hidden rounded-full bg-surface-3" data-testid="plan-progress">
      <div class="h-full rounded-full bg-[var(--status-working)]" style="width:{percent}%"></div>
    </div>
    {#if expanded}
      <ul class="flex flex-col gap-1 border-t border-border px-3 py-2 text-[13px]">
        {#if pinned}
          {@render row(pinned, true)}
        {/if}
        {#each visible as item (item.id)}
          {@render row(item, false)}
        {/each}
      </ul>
      {#if items.length > visible.length}
        <div class="px-3 pb-2 text-[12px] text-fg-3" data-testid="plan-more">
          +{items.length - visible.length} more
        </div>
      {/if}
    {/if}
  </section>
{/if}

{#snippet row(item: TodoItem, repeat: boolean)}
  <li
    class="flex gap-2 {repeat ? 'text-fg-3' : 'text-fg-2'}"
    class:line-through={item.status === 'completed' || item.status === 'cancelled'}
    class:opacity-60={item.status === 'completed' || item.status === 'cancelled'}
    data-testid="plan-row"
    data-status={item.status}
  >
    <span class="shrink-0 font-mono">{MARKS[item.status]}</span>
    <span class="shrink-0 font-mono text-fg-3">{item.id}</span>
    <span class="min-w-0">{item.content}</span>
  </li>
{/snippet}
