<script lang="ts">
  import { Combobox } from 'bits-ui';
  import type { Model } from '$lib/api/types';
  import { slideDown } from '$lib/motion';

  let { models = [], value = $bindable(''), label = 'Model', disabled = false,
    searchable = true, onchange }: {
    models?: Model[]; value?: string; label?: string; disabled?: boolean;
    searchable?: boolean; onchange?: (id: string) => void;
  } = $props();
  let search = $state('');
  let open = $state(false);
  const selected = $derived(models.find((model) => model.id === value));
  const groups = $derived.by(() => {
    const grouped = new Map<string, Model[]>();
    for (const model of models) {
      const harness = model.source === 'pi' ? 'pi-rs' : model.source === 'claude' ? 'Claude' : 'Antigravity';
      const group = `${harness} · ${model.provider}`;
      if (!`${model.id} ${model.label} ${group}`.toLowerCase().includes(search.trim().toLowerCase())) continue;
      grouped.set(group, [...(grouped.get(group) ?? []), model]);
    }
    return [...grouped].sort(([a], [b]) => a.localeCompare(b));
  });
  function windowLabel(model: Model) {
    if (!model.context_window) return 'window unknown';
    return model.context_window >= 1_000_000
      ? `${Math.round(model.context_window / 1_000_000)}M`
      : `${Math.round(model.context_window / 1000)}k`;
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
            <div {...props} transition:slideDown class="z-[70] max-h-[min(45dvh,360px)] w-[var(--bits-combobox-anchor-width)] overflow-y-auto rounded-xl border border-border bg-surface-2 p-1 shadow-xl">
              {#each groups as [group, items] (group)}
                <Combobox.Group>
                  <Combobox.GroupHeading class="sticky top-0 bg-surface-2 px-2 py-2 text-[11px] font-semibold text-fg-3">{group}</Combobox.GroupHeading>
                  {#each items as model (model.id)}
                    <Combobox.Item value={model.id} label={model.label}
                      class="cursor-pointer rounded-lg px-2 py-2 text-[13px] text-fg-1 outline-none data-highlighted:bg-surface-3 data-selected:bg-surface-3">
                      {model.label} · {windowLabel(model)}{model.stale ? ' · stale' : ''}
                    </Combobox.Item>
                  {/each}
                </Combobox.Group>
              {:else}
                <p class="px-3 py-4 text-[13px] text-fg-3">No matching models.</p>
              {/each}
            </div>
            </div>
          {/if}
        {/snippet}
      </Combobox.Content>
    </Combobox.Portal>
  </Combobox.Root>
</div>
