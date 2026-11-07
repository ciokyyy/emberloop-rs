<script lang="ts">
  import { Dialog } from 'bits-ui';
  import { fadeAt, rise, slideDown } from '$lib/motion';
  import type { ContextBreakdown, Model } from '$lib/api/types';
  import ModelPicker from './ModelPicker.svelte';
  import ContextDetails from './ContextDetails.svelte';
  import Button from './ui/button.svelte';

  let { open = $bindable(false), tab = $bindable<'model' | 'context'>('model'), models = [],
    model = '', thoughtLevel = '', used = null, size = 0, breakdown = null, disabled = false, onapply, oncache }: {
    open?: boolean; tab?: 'model' | 'context'; models?: Model[]; model?: string;
    thoughtLevel?: string; used?: number | null; size?: number; breakdown?: ContextBreakdown | null;
    disabled?: boolean; onapply: (input: { model: string; thoughtLevel: string }) => Promise<void> | void;
    oncache?: () => void;
  } = $props();
  let picked = $state('');
  let effort = $state('');
  let busy = $state(false);
  let error = $state('');
  const selected = $derived(models.find((item) => item.id === picked));
  const efforts = $derived(selected?.efforts ?? []);

  $effect(() => {
    if (open) { picked = model; effort = thoughtLevel; error = ''; }
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
        <div {...props} class="fixed inset-0 z-50 m-auto flex h-fit max-h-[calc(100dvh-2rem)] w-[min(92vw,520px)] flex-col gap-4 overflow-y-auto rounded-2xl border border-border bg-surface-2 p-5" transition:rise>
      <div class="flex items-center justify-between">
        <Dialog.Title class="text-[17px] font-semibold">Session</Dialog.Title>
        <Dialog.Close class="icon-btn" aria-label="Close session dialog">×</Dialog.Close>
      </div>
      <Dialog.Description class="sr-only">Choose a model and reasoning effort, or inspect the context sent through the proxy.</Dialog.Description>
      <div class="seg flex w-full" role="tablist" aria-label="Session details">
        <button class="flex-1" role="tab" tabindex={tab === 'context' ? 0 : -1} onkeydown={navigateTab} aria-selected={tab === 'context'} aria-controls="session-context-panel" id="session-context-tab" onclick={() => (tab = 'context')}>Context</button>
        <button class="flex-1" role="tab" tabindex={tab === 'model' ? 0 : -1} onkeydown={navigateTab} aria-selected={tab === 'model'} aria-controls="session-model-panel" id="session-model-tab" onclick={() => (tab = 'model')}>Model</button>
      </div>
      {#if tab === 'model'}
        <div id="session-model-panel" role="tabpanel" aria-labelledby="session-model-tab" in:slideDown class="flex flex-col gap-4">
          <ModelPicker searchable {models} bind:value={picked} disabled={disabled || busy} />
          {#if efforts.length}
            <label class="flex flex-col gap-1.5 text-[12px] text-fg-3">
              Reasoning effort
              <select class="field" bind:value={effort} disabled={disabled || busy}>
                {#each efforts as option (option)}<option value={option}>{option}</option>{/each}
              </select>
            </label>
          {:else}
            <p class="text-[12px] text-fg-3">This model has no configurable reasoning effort.</p>
          {/if}
          <p class="text-[12px] text-fg-3">{selected?.image_input ? 'Supports image attachments' : 'Text-only model'}{selected?.stale ? ' · Catalogue is stale' : ''}</p>
          {#if disabled}<p class="text-[12px] text-fg-3">Model settings can be changed when this Session is idle.</p>{/if}
          {#if error}<p role="alert" class="text-[12px] text-status-failed">{error}</p>{/if}
          <div class="flex justify-end gap-2">
            <Button onclick={() => (open = false)}>Cancel</Button>
            <Button variant="primary" disabled={disabled || busy} onclick={() => void apply()}>Apply</Button>
          </div>
        </div>
      {:else}
        <div id="session-context-panel" role="tabpanel" aria-labelledby="session-context-tab" in:slideDown>
          <ContextDetails {used} {size} {breakdown} {oncache} />
        </div>
      {/if}
        </div>
        {/if}
      {/snippet}
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
