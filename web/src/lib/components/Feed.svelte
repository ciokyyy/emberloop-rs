<script lang="ts">
  import type { FeedState } from '$lib/feed';
  import { renderMarkdown } from '$lib/markdown';
  import { fadeAt, rise, slideUp } from '$lib/motion';
  import ToolRow from './ToolRow.svelte';

  let { feed }: { feed: FeedState } = $props();

  let scroller: HTMLDivElement;
  let stick = $state(true);
  let open = $state<Record<string, boolean>>({});

  const routeLabel = (route?: string) =>
    route === 'injected' ? 'injected' : route === 'idle' ? 'sent at idle' : 'queued';

  // The first render and a history that lands in one update (a Session switch) appear at once; only
  // items that stream in animate. Plain sets: the effect runs before the DOM update.
  const known = new Set<string>();
  const instant = new Set<string>();
  let first = true;
  $effect.pre(() => {
    const fresh = feed.items.filter((item) => !known.has(item.id));
    if (first || fresh.length > 2) fresh.forEach((item) => instant.add(item.id));
    fresh.forEach((item) => known.add(item.id));
    first = false;
  });

  const toggle = (id: string) => (open = { ...open, [id]: !open[id] });

  function onscroll() {
    const el = scroller;
    stick = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  }

  function toTail() {
    stick = true;
    scroller.scrollTop = scroller.scrollHeight;
  }

  $effect(() => {
    // Track both the item count and the live tail text.
    feed.items.length;
    feed.items.at(-1)?.text;
    if (stick && scroller) {
      queueMicrotask(() => {
        if (scroller) scroller.scrollTop = scroller.scrollHeight;
      });
    }
  });
</script>

<div class="relative min-h-0 flex-1">
  <div bind:this={scroller} {onscroll} class="h-full overflow-y-auto px-4 py-4">
    <div class="mx-auto flex max-w-[760px] flex-col gap-1">
      {#each feed.items as item (item.id)}
        {#if item.kind === 'user'}
          <div class="my-1.5 self-end" in:slideUp|global={{ skip: instant.has(item.id) }}>
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
          <div class="md my-1.5 max-w-[92%] text-[15px]" in:rise|global={{ skip: instant.has(item.id) }}>
            {@html renderMarkdown(item.text ?? '')}
            {#if item.live}<span class="pulse text-fg-3">▍</span>{/if}
          </div>
        {:else if item.kind === 'tool'}
          <div in:rise|global={{ skip: instant.has(item.id) }}>
            <ToolRow {item} open={!!open[item.id]} ontoggle={() => toggle(item.id)} />
          </div>
        {:else if item.kind === 'compression'}
          <div
            class="my-2 flex items-center gap-2.5 rounded-xl border border-border bg-surface-1 px-3 py-2 text-[13px] text-fg-2"
            in:rise|global={{ skip: instant.has(item.id) }}
          >
            <span class="glyph text-tool-compress">⇲</span>
            <span>{item.text}</span>
          </div>
        {:else if item.kind === 'steer'}
          <div class="my-1.5 self-end text-right" in:slideUp|global={{ skip: instant.has(item.id) }}>
            <div class="inline-block max-w-[85%] rounded-2xl bg-surface-3 px-4 py-2 text-left">
              {item.text}
            </div>
            <div class="mt-0.5 text-[12px] text-fg-3">
              Steer ·
              <span class={item.route === 'pending' ? 'text-status-needs' : 'text-status-done'}>
                {routeLabel(item.route)}
              </span>
            </div>
          </div>
        {:else if item.kind === 'notice'}
          <div
            class="my-1.5 flex items-center gap-2 rounded-xl border border-border bg-surface-1 px-3 py-2 text-[13px]"
            in:fadeAt|global={{ skip: instant.has(item.id) }}
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
            in:rise|global={{ skip: instant.has(item.id) }}
          >{item.text}</div>
        {/if}
      {/each}
      {#if feed.items.length === 0}
        <p class="py-10 text-center text-[13px] text-fg-3">
          No messages yet. Send a prompt to start the Session.
        </p>
      {/if}
    </div>
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
</div>
