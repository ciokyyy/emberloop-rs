<script lang="ts">
  import { Dialog } from 'bits-ui';
  import { Check, X } from '@lucide/svelte';
  import { fadeAt, rise, slideDown } from '$lib/motion';
  import { modelIconId } from '$lib/brands';
  import { modelGroups, windowLabel } from '$lib/models';
  import type { ContextBreakdown, Model } from '$lib/api/types';
  import Brand from './Brand.svelte';
  import ContextDetails from './ContextDetails.svelte';
  import Button from './ui/button.svelte';

  let { open = $bindable(false), tab = $bindable<'model' | 'context'>('model'), models = [],
    model = '', thoughtLevel = '', used = null, size = 0, breakdown = null, disabled = false,
    sessionId = '', onapply, oncache }: {
    open?: boolean; tab?: 'model' | 'context'; models?: Model[]; model?: string;
    thoughtLevel?: string; used?: number | null; size?: number; breakdown?: ContextBreakdown | null;
    disabled?: boolean; sessionId?: string;
    onapply: (input: { model: string; thoughtLevel: string }) => Promise<void> | void;
    oncache?: () => void;
  } = $props();
  let picked = $state('');
  let effort = $state('');
  let search = $state('');
  let busy = $state(false);
  let error = $state('');
  const selected = $derived(models.find((item) => item.id === picked));
  const efforts = $derived(selected?.efforts ?? []);
  // The models sit in the dialog itself: a list opens under the composer, not one popup inside another.
  const groups = $derived(modelGroups(models, search));

  $effect(() => {
    if (open) { picked = model; effort = thoughtLevel; search = ''; error = ''; }
  });
  $effect(() => {
    if (!efforts.includes(effort)) effort = efforts[0] ?? '';
  });

  function navigateTab(event: KeyboardEvent) {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    tab = event.key === 'Home' ? 'context' : event.key === 'End' ? 'model' : tab === 'model' ? 'context' : 'model';
    (event.currentTarget as HTMLElement).parentElement?.querySelector<HTMLButtonElement>(`#session-${tab}-tab`)?.focus();
  }

  async function apply() {
    busy = true;
    error = '';
    try {
      await onapply({ model: picked, thoughtLevel: effort });
      open = false;
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally { busy = false; }
  }
</script>


<Dialog.Root bind:open>
  <Dialog.Portal>
    <Dialog.Overlay forceMount>
      {#snippet child({ props })}
        {#if open}<div {...props} class="fixed inset-0 z-40 bg-black/60" transition:fadeAt></div>{/if}
      {/snippet}
    </Dialog.Overlay>
    <Dialog.Content forceMount>
      {#snippet child({ props })}
        {#if open}
        <div {...props} class="fixed inset-0 z-50 m-auto flex h-fit max-h-[calc(100dvh-2rem)] w-[min(92vw,560px)] flex-col overflow-hidden rounded-2xl border border-border bg-surface-1 shadow-xl" transition:rise>
      <div class="flex items-center justify-between gap-1 px-3 pt-3 pb-2">
        <Dialog.Title class="text-[13px] font-semibold text-fg-1">Session</Dialog.Title>
        <Dialog.Close class="icon-btn" aria-label="Close session dialog"><X class="size-4" /></Dialog.Close>
      </div>
      <Dialog.Description class="sr-only">Choose a model and reasoning effort, or inspect the context sent through the proxy.</Dialog.Description>
      <div class="px-3">
        <div class="flex gap-5 border-b border-border" role="tablist" aria-label="Session details">
          <button class="-mb-px border-b-2 border-transparent pt-1 pb-2 text-[13px] text-fg-3 aria-selected:border-fg-1 aria-selected:font-semibold aria-selected:text-fg-1" role="tab" tabindex={tab === 'context' ? 0 : -1} onkeydown={navigateTab} aria-selected={tab === 'context'} aria-controls="session-context-panel" id="session-context-tab" onclick={() => (tab = 'context')}>Context</button>
          <button class="-mb-px border-b-2 border-transparent pt-1 pb-2 text-[13px] text-fg-3 aria-selected:border-fg-1 aria-selected:font-semibold aria-selected:text-fg-1" role="tab" tabindex={tab === 'model' ? 0 : -1} onkeydown={navigateTab} aria-selected={tab === 'model'} aria-controls="session-model-panel" id="session-model-tab" onclick={() => (tab = 'model')}>Model</button>
        </div>
      </div>
      {#if tab === 'model'}
        <div id="session-model-panel" role="tabpanel" aria-labelledby="session-model-tab" in:slideDown class="flex min-h-0 flex-1 flex-col">
          <div class="px-3 pt-3">
            <input class="field text-[13px]" aria-label="Search models" placeholder="Search models…" bind:value={search} />
          </div>
          <div class="scroll-thin min-h-0 flex-1 overflow-y-auto px-3 pt-2 pb-2">
            {#if groups.length === 0}
              <p class="px-2 py-6 text-[13px] text-fg-3">No matching models.</p>
            {:else}
              {#each groups as [heading, items] (heading)}
                <section class="flex flex-col">
                  <p class="px-3 pt-2 pb-1 text-[10px] font-semibold uppercase tracking-[0.05em] text-fg-3">{heading}</p>
                  {#each items as item (item.id)}
                    <button type="button" aria-pressed={picked === item.id} disabled={disabled || busy}
                      class="row flex w-full items-center gap-2 px-2 py-2 text-left {picked === item.id ? 'sel' : ''}"
                      onclick={() => (picked = item.id)}>
                      <Brand id={modelIconId(item)} size={16} />
                      <span class="min-w-0 flex-1 truncate text-[13px] text-fg-1">{item.label}</span>
                      <span class="shrink-0 text-[12px] tabular-nums text-fg-3">{windowLabel(item)}{item.stale ? ' · stale' : ''}</span>
                      {#if picked === item.id}<Check class="size-4 shrink-0 text-brand" />{/if}
                    </button>
                  {/each}
                </section>
              {/each}
            {/if}
          </div>
          <div class="flex flex-col gap-2 border-t border-border px-3 py-3">
            {#if efforts.length}
              <div class="flex flex-col gap-1.5">
                <span class="text-[12px] text-fg-3">Reasoning effort</span>
                <div class="seg w-fit" role="group" aria-label="Reasoning effort">
                  {#each efforts as option (option)}
                    <button type="button" aria-pressed={effort === option} disabled={disabled || busy} onclick={() => (effort = option)}>{option}</button>
                  {/each}
                </div>
              </div>
            {:else}
              <p class="text-[12px] text-fg-3">This model has no configurable reasoning effort.</p>
            {/if}
            <p class="text-[12px] text-fg-3">{selected?.image_input ? 'Supports image attachments' : 'Text-only model'}{selected?.stale ? ' · catalogue is stale' : ''}</p>
            {#if disabled}<p class="text-[12px] text-fg-3">Model settings can be changed when this Session is idle.</p>{/if}
            {#if error}<p role="alert" class="text-[12px] text-status-failed">{error}</p>{/if}
          </div>
        </div>
        <div class="flex justify-end gap-2 border-t border-border px-3 py-2">
          <Button onclick={() => (open = false)}>Cancel</Button>
          <Button variant="primary" disabled={disabled || busy} onclick={() => void apply()}>Apply</Button>
        </div>
      {:else}
        <div id="session-context-panel" role="tabpanel" aria-labelledby="session-context-tab" in:slideDown class="scroll-thin min-h-0 flex-1 overflow-y-auto px-3 py-3">
          <ContextDetails {used} {size} {breakdown} {oncache} {sessionId} />
        </div>
      {/if}
        </div>
        {/if}
      {/snippet}
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
