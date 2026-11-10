<script lang="ts">
  import { tick } from 'svelte';
  import { get } from 'svelte/store';
  import { createVirtualizer } from '@tanstack/svelte-virtual';
  import type { FeedItem, FeedState } from '$lib/feed';
  import { renderMarkdown } from '$lib/markdown';
  import { fadeAt, rise, slideUp } from '$lib/motion';
  import { segments } from '$lib/toolRuns';
  import Icon from './Icon.svelte';
  import ToolFeed from './ToolFeed.svelte';
  import SubagentReport from './SubagentReport.svelte';

  let {
    feed,
    projectId = '',
    hasMore = false,
    loadingMore = false,
    onloadmore = null
  }: {
    feed: FeedState;
    projectId?: string;
    hasMore?: boolean;
    loadingMore?: boolean;
    onloadmore?: (() => Promise<void>) | null;
  } = $props();
  let scroller: HTMLDivElement;
  let stick = $state(true);

  const routeLabel = (route?: string) =>
    route === 'injected'
      ? 'injected'
      : route === 'idle'
        ? 'sent at idle'
        : route === 'cap'
          ? 'reminder cap'
          : 'queued';
  // A todo reminder is a dashed note; the cap note keeps the same shape.
  const isReminder = (item: FeedItem) =>
    item.route === 'cap' || (item.text ?? '').startsWith('Reminder ');

  // Only an arrival animates. A row the virtual window remounts on the way back up has been on
  // screen before, so `arrived` holds just the items this update added; plain sets, because the
  // effect runs before the DOM update that reads them.
  const known = new Set<string>();
  let arrived = $state<Set<string>>(new Set());
  let first = true;
  $effect.pre(() => {
    const fresh = feed.items.filter((item) => !known.has(item.id));
    // The first render and a history that lands in one update (a Session switch or an older page)
    // appear at once; only items that stream in animate.
    arrived = first || fresh.length > 2 ? new Set() : new Set(fresh.map((item) => item.id));
    fresh.forEach((item) => known.add(item.id));
    first = false;
  });

  // A queued steer waits at the bottom of the feed, out of the run it is about to join; the delivery
  // event drops it back into its own place in the log.
  const pendingSteer = (item: FeedItem) => item.kind === 'steer' && item.route === 'pending';
  const pinned = $derived(feed.items.filter(pendingSteer));
  const segs = $derived(segments(feed.items.filter((item) => !pendingSteer(item))));

  // Fold, Restore, Skill review and Context rows open in place, like the tool rows around them.
  let opened = $state<Set<string>>(new Set());
  const toggle = (id: string) =>
    (opened = new Set(opened.has(id) ? [...opened].filter((key) => key !== id) : [...opened, id]));

  // Rows are absolutely placed by the virtualizer, so only the visible window is in the DOM and a
  // Session with thousands of blocks scrolls like a short one.
  const virtualizer = createVirtualizer({
    count: 0,
    getScrollElement: () => scroller,
    estimateSize: () => 110,
    overscan: 6,
    getItemKey: (index: number) => {
      const seg = segs[index];
      return seg ? (seg.type === 'item' ? seg.item.id : seg.id) : index;
    }
  });
  const rows = $derived($virtualizer.getVirtualItems());
  const total = $derived($virtualizer.getTotalSize());

  // setOptions writes the virtualizer store, so it is read unsubscribed here: subscribing in the
  // same effect would re-run it on its own write.
  $effect(() => {
    get(virtualizer).setOptions({ count: segs.length });
  });

  function onscroll() {
    const el = scroller;
    stick = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  }

  /** Register a mounted row with the virtualizer so its real height is recorded, and drop it on
   *  unmount. An action, because a row element is what the API wants, not a bindable. */
  const measureRow = (node: HTMLElement) => {
    const instance = get(virtualizer);
    instance.measureElement(node);
    return { destroy: () => instance.measureElement(null) };
  };

  function toTail() {
    stick = true;
    scroller.scrollTop = scroller.scrollHeight;
  }

  // A page arrives above the viewport, so the height that grew is added to scrollTop: the row that
  // was under the eye stays under the eye instead of the feed jumping to the top.
  async function loadEarlier() {
    const el = scroller;
    if (!el || !onloadmore) return;
    stick = false;
    const before = el.scrollHeight;
    await onloadmore();
    await tick();
    el.scrollTop += el.scrollHeight - before;
  }

  $effect(() => {
    // Track both the item count and the live tail text.
    feed.items.length;
    feed.items.at(-1)?.text;
    if (stick && scroller) {
      queueMicrotask(() => {
        if (stick && scroller) scroller.scrollTop = scroller.scrollHeight;
      });
    }
  });
</script>

<div class="relative min-h-0 flex-1">
  {#if hasMore}
    <button
      class="btn absolute top-2 left-1/2 z-10 -translate-x-1/2 text-[12px] shadow-lg"
      onclick={() => void loadEarlier()}
      disabled={loadingMore}
      data-testid="load-earlier"
    >
      {loadingMore ? 'Loading…' : 'Load earlier'}
    </button>
  {/if}

  <div
    bind:this={scroller}
    {onscroll}
    data-testid="feed-scroller"
    class="h-full overflow-y-auto px-4 py-4"
  >
    <!-- Rows stay in normal flow between two spacers. Absolute placement overlapped rows whenever a
         measured height lagged the DOM, so a window of real rows never stacks on another row. -->
    <div
      class="mx-auto max-w-[760px]"
      style="padding-top:{rows[0]?.start ?? 0}px;padding-bottom:{total - (rows.at(-1)?.end ?? 0)}px"
    >
      {#each rows as row (row.key)}
        {@const seg = segs[row.index]}
        {#if seg}
          <div class="flex min-w-0 flex-col pb-0.5" data-index={row.index} use:measureRow>
            {@render segRow(seg)}
          </div>
        {/if}
      {/each}
    </div>

    <!-- A queued steer keeps the log's column, so it never grows past the rows it joins. -->
    <div class="mx-auto flex max-w-[760px] flex-col">
      {#each pinned as item (item.id)}
        {@render steerBubble(item)}
      {/each}
    </div>
    {#if feed.items.length === 0}
      <p class="py-10 text-center text-[13px] text-fg-3">
        No messages yet. Send a prompt to start the Session.
      </p>
    {/if}
  </div>

  {#if !stick}
    <button
      class="btn absolute bottom-3 left-1/2 -translate-x-1/2 shadow-lg"
      onclick={toTail}
      transition:fadeAt
    >
      Jump to latest ↓
    </button>
  {/if}

{#snippet segRow(seg: ReturnType<typeof segments>[number])}
  {#if seg.type === 'tools'}
    <div class="min-w-0" in:rise|global={{ skip: !arrived.has(seg.id) }}>
      <ToolFeed runs={seg.runs} />
    </div>
  {:else}
    {@const item = seg.item}
    {#if item.kind === 'user'}
      <div class="my-1.5 self-end" in:slideUp|global={{ skip: !arrived.has(item.id) }}>
        <div class="max-w-[85vw] rounded-2xl bg-surface-3 px-4 py-2.5 sm:max-w-[85%]">
          <p class="whitespace-pre-wrap">{item.text}</p>
          {#if item.imagesCount}
            <div class="mt-2 flex gap-2">
              <div
                class="grid h-16 w-16 place-items-center rounded-lg border border-border bg-surface-1 px-1 text-center text-[11px] text-fg-3"
              >{item.imagesCount} image{item.imagesCount === 1 ? '' : 's'}</div>
            </div>
          {/if}
        </div>
      </div>
    {:else if item.kind === 'assistant'}
      <div
        class="md my-1.5 max-w-[92%] text-[15px]"
        in:rise|global={{ skip: !arrived.has(item.id) }}
      >
        {@html renderMarkdown(item.text ?? '')}
        {#if item.live}<span class="pulse text-fg-3">▍</span>{/if}
      </div>
    {:else if item.kind === 'compression'}
      {@const label = item.action === 'skill-review' ? 'Skill review' : 'Context'}
      <div class="my-0.5" in:rise|global={{ skip: !arrived.has(item.id) }}>
        <button
          type="button"
          class="flex w-full items-center gap-2 rounded px-2 py-1 text-left select-none hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[28px] sm:min-h-[28px] min-h-[44px]"
          aria-expanded={opened.has(item.id)}
          onclick={() => toggle(item.id)}
        >
          <span
            class="h-[7px] w-[7px] rounded-full bg-[var(--tool-compress)] shrink-0"
            aria-hidden="true"
          ></span>
          <span class="font-medium text-[var(--fg-3)] shrink-0">{label}</span>
          <span class="font-mono text-[12.5px] text-[var(--fg-2)] truncate flex-1">{item.text}</span>
          {#if item.meta}
            <span class="text-xs text-[var(--fg-4)] tabular-nums shrink-0">{item.meta}</span>
          {/if}
        </button>
        {#if opened.has(item.id)}
          <div
            class="expanded-box min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] bg-[#1c1c1e] border border-[#2c2c2e] rounded-lg p-2.5 my-1 font-mono text-[11.5px] text-[var(--fg-1)] whitespace-pre-wrap"
          >{item.body || item.text || '(no details)'}</div>
        {/if}
      </div>
    {:else if item.kind === 'steer'}
      {@render steerBubble(item)}
    {:else if item.kind === 'notice'}
      <div
        class="my-1.5 flex items-center gap-2 rounded-xl border bg-surface-1 px-3 py-2 text-[13px] {isReminder(
          item
        )
          ? 'border-dashed border-fg-3'
          : 'border-border'}"
        data-reminder={isReminder(item) ? 'true' : undefined}
        in:fadeAt|global={{ skip: !arrived.has(item.id) }}
      >
        <span class="min-w-0 flex-1">{item.text}</span>
        <span
          class={`chip shrink-0 whitespace-nowrap ${item.route === 'pending' ? 'text-status-needs' : 'text-status-done'}`}
        >{routeLabel(item.route)}</span>
      </div>
    {:else if item.kind === 'error'}
      <div
        class="my-2 rounded-xl border border-status-failed px-3 py-2 text-[13px]"
        style="background:#2a2222;color:#ffb3ad"
        in:rise|global={{ skip: !arrived.has(item.id) }}
      >{item.text}</div>
    {/if}
  {/if}
{/snippet}

{#snippet steerBubble(item: FeedItem)}
  {#if item.report}
    <SubagentReport report={item.report} route={item.route} {projectId} />
  {:else}
  <div class="my-1.5 self-end text-right" in:slideUp|global={{ skip: !arrived.has(item.id) }}>
    <div class="inline-block max-w-[85%] rounded-2xl bg-surface-3 px-4 py-2 text-left">
      {item.text}
      {#if item.imagesCount}
        <div class="mt-1 text-[11px] text-fg-3">
          {item.imagesCount} image{item.imagesCount === 1 ? '' : 's'}
        </div>
      {/if}
    </div>
    <div class="mt-0.5 text-[12px] text-fg-3">
      Steer ·
      <span class={item.route === 'pending' ? 'text-status-needs' : 'text-status-done'}>
        {routeLabel(item.route)}
      </span>
    </div>
  </div>
  {/if}
{/snippet}

</div>
