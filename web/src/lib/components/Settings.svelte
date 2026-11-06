<script lang="ts">
  import type { Model, Settings } from '$lib/api/types';
  import { rise } from '$lib/motion';
  import ModelPicker from './ModelPicker.svelte';
  import Prompts from './Prompts.svelte';
  import Switch from './ui/switch.svelte';

  let {
    settings,
    models = [],
    saving = false,
    onsave
  }: {
    settings: Settings;
    models?: Model[];
    saving?: boolean;
    onsave: (settings: Settings) => void;
  } = $props();

  const levels = ['off', 'lite', 'full', 'ultra'];
  const subagents = [
    { name: 'scout', description: 'Reads and searches. Cannot edit.', tools: 'read, grep' },
    { name: 'worker', description: 'Edits and runs commands. One at a time per Session.', tools: 'all tools except subagents' }
  ];
</script>

<div class="h-full overflow-y-auto px-4 py-6" in:rise|global>
  <div class="mx-auto flex max-w-[760px] flex-col gap-8">
    <div>
      <h2 class="text-[1.75rem] font-semibold leading-[1.2] tracking-[-0.02em]">Settings</h2>
      <p class="text-fg-3">
        Daemon-wide. Changes apply to the next Session, not to running ones.
        {#if saving}<span class="text-fg-3"> Saving…</span>{/if}
      </p>
    </div>

    <section class="flex flex-col gap-3">
      <h3 class="text-[1.0625rem] font-semibold">Proxy</h3>
      <div class="flex flex-col gap-4 rounded-2xl border border-border bg-surface-1 p-4">
        <div class="flex items-center justify-between gap-4">
          <div>
            <div>Billion context</div>
            <div class="text-[13px] text-fg-3">
              Compress, decompress and search the history through the proxy.
            </div>
          </div>
          <Switch
            checked={settings.proxy_compression}
            label="Billion context"
            onchange={(checked) => onsave({ ...settings, proxy_compression: checked })}
          />
        </div>
        <div class="flex flex-wrap items-center justify-between gap-4">
          <div>
            <div>Ponytail level</div>
            <div class="text-[13px] text-fg-3">
              Default for new Sessions. One embedded skill text, appended by the proxy.
            </div>
          </div>
          <div class="seg">
            {#each levels as level (level)}
              <button
                aria-pressed={settings.ponytail_default_level === level}
                onclick={() => onsave({ ...settings, ponytail_default_level: level })}
              >{level}</button>
            {/each}
          </div>
        </div>
      </div>
    </section>

    <Prompts />

    <section class="flex flex-col gap-3">
      <div class="flex items-center gap-2">
        <h3 class="text-[1.0625rem] font-semibold">Subagents</h3>
        <span class="chip">phase 2</span>
      </div>
      <p class="text-[13px] text-fg-3">
        scout reads and searches. worker edits. Both are off until phase 2; the form shows what will
        be configurable.
      </p>
      <div class="grid gap-3 sm:grid-cols-2">
        {#each subagents as agent (agent.name)}
          <div class="flex flex-col gap-3 rounded-2xl border border-border bg-surface-1 p-4 opacity-70">
            <div class="flex items-start justify-between gap-3">
              <div>
                <div class="font-mono font-semibold">{agent.name}</div>
                <div class="text-[13px] text-fg-3">{agent.description}</div>
              </div>
              <Switch checked={false} disabled label={`Enable ${agent.name}`} />
            </div>
            <ModelPicker {models} label="Model" disabled />
            <label class="flex flex-col gap-1 text-[12px] text-fg-3">
              Extra instruction
              <input class="field" disabled placeholder="Optional line added to its prompt" />
            </label>
            <div class="text-[12px] text-fg-3">Tools: {agent.tools}</div>
          </div>
        {/each}
      </div>
    </section>
  </div>
</div>
