<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { Model, Settings, ToolKey } from '$lib/api/types';
  import { rise } from '$lib/motion';
  import ModelPicker from './ModelPicker.svelte';
  import Prompts from './Prompts.svelte';
  import Button from './ui/button.svelte';
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

  // Tool keys live in the runtime env file the tool shells source, so a saved key applies to the
  // next command with no restart. The API never returns a stored value, only whether one is set.
  let toolKeys = $state<ToolKey[]>([]);
  let drafts = $state<Record<string, string>>({});
  let toolError = $state('');
  let toolSaving = $state('');
  onMount(() => void loadToolKeys());

  async function loadToolKeys() {
    try {
      toolKeys = await api.listToolKeys();
      toolError = '';
    } catch (cause) {
      toolError = cause instanceof Error ? cause.message : String(cause);
    }
  }

  async function saveToolKey(name: string) {
    toolSaving = name;
    try {
      await api.setToolKey(name, drafts[name] ?? '');
      drafts = { ...drafts, [name]: '' };
      await loadToolKeys();
    } catch (cause) {
      toolError = cause instanceof Error ? cause.message : String(cause);
    } finally {
      toolSaving = '';
    }
  }

  const labels: Record<string, string> = { brave: 'Brave search', exa: 'Exa search', jina: 'Jina reader' };

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
      <h3 class="text-[1.0625rem] font-semibold">Tool keys</h3>
      <div class="flex flex-col gap-4 rounded-2xl border border-border bg-surface-1 p-4">
        <p class="text-[13px] text-fg-3">
          Saved here, not exported in a shell: the tool shells read them from the runtime env file
          on their next command. An empty value removes a key.
        </p>
        {#each toolKeys as key (key.name)}
          <div class="flex items-end gap-2">
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-[12px] text-fg-3">
              {labels[key.name] ?? key.name} <span class="font-mono text-fg-4">{key.variable}</span>
              ({key.set ? 'set' : 'not set'})
              <input
                class="field"
                type="password"
                autocomplete="off"
                placeholder={key.set ? 'Replace saved key' : 'Paste key'}
                bind:value={drafts[key.name]}
              />
            </label>
            <Button
              disabled={toolSaving === key.name || !(drafts[key.name] ?? '').trim()}
              onclick={() => void saveToolKey(key.name)}
            >Save</Button>
          </div>
        {/each}
        {#if toolError}<p class="text-[13px] text-status-failed">{toolError}</p>{/if}
      </div>
    </section>

    <section class="flex flex-col gap-3">
      <h3 class="text-[1.0625rem] font-semibold">Session titles</h3>
      <div class="flex flex-col gap-4 rounded-2xl border border-border bg-surface-1 p-4">
        <label class="flex items-center justify-between gap-4">
          Generate titles automatically
          <Switch label="Generate titles automatically" checked={settings.title_generation} onchange={(title_generation) => onsave({ ...settings, title_generation })} />
        </label>
        <ModelPicker label="Title model" models={[{ id: 'openrouter/openrouter/free', label: 'OpenRouter Free', provider: 'openrouter', source: 'pi', kind: 'chat', efforts: [], image_input: false, context_window: 0, stale: false, updated_at: 0 }, ...models.filter((model) => model.source === 'pi' && model.id !== 'openrouter/openrouter/free')]} value={settings.title_model} onchange={(title_model) => onsave({ ...settings, title_model })} />
        <p class="text-[13px] text-fg-3">Uses saved provider credentials. Retries until saved. Manual titles stay unchanged.</p>
      </div>
    </section>

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

    <section class="flex flex-col gap-3">
      <h3 class="text-[1.0625rem] font-semibold">Context</h3>
      <div class="grid gap-4 rounded-2xl border border-border bg-surface-1 p-4 sm:grid-cols-2">
        <label class="flex flex-col gap-1 text-[13px]">
          Pressure band (%)
          <input
            class="field"
            type="number"
            min="1"
            max="100"
            value={settings.max_context_limit}
            onchange={(event) =>
              onsave({
                ...settings,
                max_context_limit: Math.max(1, Number((event.currentTarget as HTMLInputElement).value) || 75)
              })}
          />
          <span class="text-[12px] text-fg-3">maxContextLimit; a nudge every turn while over.</span>
        </label>
        <label class="flex flex-col gap-1 text-[13px]">
          Emergency band (%)
          <input
            class="field"
            type="number"
            min="1"
            max="100"
            value={settings.emergency_threshold_percent}
            onchange={(event) =>
              onsave({
                ...settings,
                emergency_threshold_percent: Math.max(
                  1,
                  Number((event.currentTarget as HTMLInputElement).value) || 95
                )
              })}
          />
          <span class="text-[12px] text-fg-3">emergencyThresholdPercent; truncates large tool output.</span>
        </label>
        <label class="flex flex-col gap-1 text-[13px]">
          Pressure floor (tokens)
          <input
            class="field"
            type="number"
            min="0"
            value={settings.min_pressure_benefit_tokens}
            onchange={(event) =>
              onsave({
                ...settings,
                min_pressure_benefit_tokens: Math.max(
                  0,
                  Number((event.currentTarget as HTMLInputElement).value) || 0
                )
              })}
          />
          <span class="text-[12px] text-fg-3">minPressureBenefitTokens; 0 means auto.</span>
        </label>
        <label class="flex flex-col gap-1 text-[13px]">
          Preserved recent messages
          <input
            class="field"
            type="number"
            min="0"
            value={settings.preserve_recent_messages}
            onchange={(event) =>
              onsave({
                ...settings,
                preserve_recent_messages: Math.max(
                  0,
                  Number((event.currentTarget as HTMLInputElement).value) || 0
                )
              })}
          />
          <span class="text-[12px] text-fg-3">preserveRecentMessages; the tail never folds.</span>
        </label>
        <label class="flex flex-col gap-1 text-[13px]">
          Tier 2 blocks
          <input
            class="field"
            type="number"
            min="1"
            value={settings.tier2_trigger}
            onchange={(event) =>
              onsave({
                ...settings,
                tier2_trigger: Math.max(1, Number((event.currentTarget as HTMLInputElement).value) || 1000)
              })}
          />
        </label>
        <label class="flex flex-col gap-1 text-[13px]">
          Tier 3 blocks
          <input
            class="field"
            type="number"
            min="1"
            value={settings.tier3_trigger}
            onchange={(event) =>
              onsave({
                ...settings,
                tier3_trigger: Math.max(1, Number((event.currentTarget as HTMLInputElement).value) || 2000)
              })}
          />
        </label>
        <label class="flex flex-col gap-1 text-[13px] sm:col-span-2">
          Protected tools
          <input
            class="field"
            value={settings.protected_tools.join(', ')}
            placeholder="comma separated, e.g. read, bash"
            onchange={(event) =>
              onsave({
                ...settings,
                protected_tools: (event.currentTarget as HTMLInputElement).value
                  .split(',')
                  .map((name) => name.trim())
                  .filter(Boolean)
              })}
          />
          <span class="text-[12px] text-fg-3">protectedTools; their results never fold.</span>
        </label>
        <div class="flex items-center justify-between gap-4">
          <div>
            <div>Inject tools</div>
            <div class="text-[13px] text-fg-3">Declare compress, decompress, search and status.</div>
          </div>
          <Switch
            checked={settings.inject_tool}
            label="Inject tools"
            onchange={(checked) => onsave({ ...settings, inject_tool: checked })}
          />
        </div>
        <div class="flex items-center justify-between gap-4">
          <div>
            <div>Inject nudges</div>
            <div class="text-[13px] text-fg-3">Ask the model to compress at the bands above.</div>
          </div>
          <Switch
            checked={settings.inject_nudge}
            label="Inject nudges"
            onchange={(checked) => onsave({ ...settings, inject_nudge: checked })}
          />
        </div>
      </div>
    </section>
    <section class="flex flex-col gap-3">
      <h3 class="text-[1.0625rem] font-semibold">Todo</h3>
      <div class="flex flex-col gap-4 rounded-2xl border border-border bg-surface-1 p-4">
        <div class="flex items-center justify-between gap-4">
          <div>
            <div>Stop reminders</div>
            <div class="text-[13px] text-fg-3">
              Wake the model when a turn ends with items still open.
            </div>
          </div>
          <Switch
            checked={settings.todo_reminders}
            label="Stop reminders"
            onchange={(checked) => onsave({ ...settings, todo_reminders: checked })}
          />
        </div>
        <label class="flex flex-wrap items-center justify-between gap-4">
          <span>
            <span>Reminders per turn</span>
            <span class="block text-[13px] text-fg-3">
              Reset by the next message; 0 turns reminders off.
            </span>
          </span>
          <input
            class="field w-20"
            type="number"
            min="0"
            max="10"
            value={settings.todo_reminders_max}
            onchange={(event) =>
              onsave({
                ...settings,
                todo_reminders_max: Math.max(0, Number((event.currentTarget as HTMLInputElement).value) || 0)
              })}
          />
        </label>
        <div class="flex items-center justify-between gap-4">
          <div>
            <div>Open the plan on create</div>
            <div class="text-[13px] text-fg-3">The card expands for two seconds when the list changes.</div>
          </div>
          <Switch
            checked={settings.todo_auto_expand}
            label="Open the plan on create"
            onchange={(checked) => onsave({ ...settings, todo_auto_expand: checked })}
          />
        </div>
        <div class="flex items-center justify-between gap-4">
          <div>
            <div>Fade when done</div>
            <div class="text-[13px] text-fg-3">
              The card fades a few seconds after the last item resolves.
            </div>
          </div>
          <Switch
            checked={settings.todo_hide_when_done}
            label="Fade when done"
            onchange={(checked) => onsave({ ...settings, todo_hide_when_done: checked })}
          />
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
