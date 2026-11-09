<script lang="ts">
  import type { Model } from '$lib/api/types';
  import { chosenWindow } from '$lib/contextMeter';
  import { modelIconId } from '$lib/brands';
  import Composer from './Composer.svelte';
  import SessionDialog from './SessionDialog.svelte';

  let { sessionId, models, model = $bindable(''), onsubmit }: {
    sessionId: string;
    models: Model[];
    model?: string;
    onsubmit: (text: string, files: File[], thoughtLevel: string) => Promise<void> | void;
  } = $props();
  let open = $state(false);
  let tab = $state<'model' | 'context'>('model');
  let effort = $state('');
  const selected = $derived(models.find((entry) => entry.id === model));
  const size = $derived(chosenWindow(selected?.context_window, 0));
  $effect(() => {
    if (!selected?.efforts.includes(effort)) effort = selected?.efforts[0] ?? '';
  });
</script>

<div class="grid flex-1 place-items-center px-4">
  <h1 class="text-center text-2xl font-semibold">What are we working on today?</h1>
</div>
<Composer {sessionId} model={selected?.label ?? model} contextSize={size}
  providerIcon={selected ? modelIconId(selected) : ''} imageInput={selected?.image_input ?? false}
  ondetails={(next) => { tab = next; open = true; }}
  onsubmit={(text, files) => onsubmit(text, files, effort)} oncancel={() => {}} />
<SessionDialog bind:open bind:tab {models} {model} thoughtLevel={effort}
  used={null} {size} breakdown={null}
  onapply={(pick) => { model = pick.model; effort = pick.thoughtLevel; }} oncache={() => {}} />
