<script lang="ts">
  import { Combobox } from 'bits-ui';
  import { createVirtualizer } from '@tanstack/svelte-virtual';
  import { get } from 'svelte/store';
  import { modelIconId } from '$lib/brands';
  import { modelGroups, windowLabel } from '$lib/models';
  import type { Model } from '$lib/api/types';
  import { slideDown } from '$lib/motion';
  import Brand from './Brand.svelte';

  let { models = [], value = $bindable(''), label = 'Model', disabled = false,
    searchable = true, onchange }: {
    models?: Model[]; value?: string; label?: string; disabled?: boolean;
    searchable?: boolean; onchange?: (id: string) => void;
  } = $props();
  let search = $state('');
  let open = $state(false);
  const selected = $derived(models.find((model) => model.id === value));
  const groups = $derived(modelGroups(models, search));
  // Headings and items in one flat list, so the virtualizer windows both.
  type Row = { key: string; label: string; model?: Model };
  const rows = $derived<Row[]>(groups.flatMap(([group, items]) => [
    { key: `h:${group}`, label: group },
    ...items.map((model) => ({ key: `m:${model.id}`, label: model.label, model }))
  ]));

  let viewport: HTMLElement | null = null;
  const virtualizer = createVirtualizer({
    count: 0,
    getScrollElement: () => viewport,
    estimateSize: (index) => (rows[index]?.model ? 36 : 28),
    overscan: 6,
    getItemKey: (index) => rows[index]?.key ?? index
  });
  // The viewport mounts only while open; re-sync the virtualizer count when rows or open change.
  $effect(() => {
    void open;
    get(virtualizer).setOptions({ count: rows.length });
  });

  // Bits-ui moves the highlight over rows the window has not rendered; scroll the window to the row it lands on.
  // A pointer sets the same highlight, and following it would drag a touch scroll back to the row under the finger.
  function viewportAction(node: HTMLElement) {
    viewport = node;
    let byPointer = false;
    const pointer = () => { byPointer = true; };
    const key = () => { byPointer = false; };
    node.addEventListener('pointermove', pointer);
    node.addEventListener('pointerdown', pointer);
    document.addEventListener('keydown', key, true);
    const observer = new MutationObserver(() => {
      if (byPointer) return;
      const index = node.querySelector<HTMLElement>('[data-highlighted]')?.closest<HTMLElement>('[data-index]')?.dataset.index;
      if (index !== undefined) get(virtualizer).scrollToIndex(Number(index), { align: 'auto' });
    });
    observer.observe(node, { subtree: true, attributes: true, attributeFilter: ['data-highlighted'] });
    return {
      destroy: () => {
        observer.disconnect();
        node.removeEventListener('pointermove', pointer);
        node.removeEventListener('pointerdown', pointer);
        document.removeEventListener('keydown', key, true);
        viewport = null;
      }
    };
  }
</script>

<div class="flex flex-col gap-1.5 text-[12px] text-fg-3">
  <span>{label}</span>
  <Combobox.Root type="single" bind:value bind:open {disabled} allowDeselect={false}
    inputValue={open ? search : selected?.label ?? value}
    onOpenChange={(opened) => { if (!opened) search = ''; }}
    onValueChange={(id) => { search = ''; onchange?.(id); }}>
    <div class="relative flex items-center">
      <Combobox.Input aria-label={label} class="field w-full pr-9" placeholder="Search models…"
        readonly={!searchable} oninput={(event) => { search = event.currentTarget.value; open = true; }} />
      <Combobox.Trigger aria-label={`Show ${label.toLowerCase()} options`} class="absolute right-2 rounded px-1 py-1 hover:bg-surface-3">⌄</Combobox.Trigger>
    </div>
    <Combobox.Portal>
      <Combobox.Content forceMount sideOffset={6} class="z-[70] w-[var(--bits-combobox-anchor-width)] rounded-xl border border-border bg-surface-2 shadow-xl">
        {#snippet child({ props, wrapperProps })}
          {#if open}
            <div {...wrapperProps} class="z-[70]">
              <div {...props} use:viewportAction data-testid="model-viewport" transition:slideDown class="z-[70] max-h-[min(45dvh,360px)] w-[var(--bits-combobox-anchor-width)] overflow-y-auto rounded-xl border border-border bg-surface-2 p-1 shadow-xl">
                {#if rows.length === 0}
                  <p class="px-3 py-4 text-[13px] text-fg-3">No matching models.</p>
                {:else}
                  <div class="relative w-full" style:height="{$virtualizer.getTotalSize()}px">
                    {#each $virtualizer.getVirtualItems() as row (row.key)}
                      {@const entry = rows[row.index]}
                      <div class="absolute left-0 top-0 w-full" data-index={row.index} style:transform="translateY({row.start}px)" style:height="{row.size}px">
                        {#if entry?.model}
                          <Combobox.Item value={entry.model.id} label={entry.model.label}
                            class="flex h-full cursor-pointer items-center gap-2 rounded-lg px-2 text-[13px] text-fg-1 outline-none data-highlighted:bg-surface-3 data-selected:bg-surface-3">
                            <Brand id={modelIconId(entry.model)} size={16} />
                            <span class="min-w-0 truncate">{entry.model.label} · {windowLabel(entry.model)}{entry.model.stale ? ' · stale' : ''}</span>
                          </Combobox.Item>
                        {:else if entry}
                          <div class="flex h-full items-end px-2 pb-1 text-[11px] font-semibold text-fg-3">{entry.label}</div>
                        {/if}
                      </div>
                    {/each}
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        {/snippet}
      </Combobox.Content>
    </Combobox.Portal>
  </Combobox.Root>
</div>
