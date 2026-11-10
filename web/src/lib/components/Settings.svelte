<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { Model, Settings, ToolKey } from '$lib/api/types';
  import { rise } from '$lib/motion';
  import ModelPicker from './ModelPicker.svelte';
  import Prompts from './Prompts.svelte';
  import Button from './ui/button.svelte';
  import Slider from './ui/slider.svelte';
  import Switch from './ui/switch.svelte';

  let {
    settings: saved,
    models = [],
    saving = false,
    onsave
  }: {
    settings: Settings;
    models?: Model[];
    saving?: boolean;
    onsave: (settings: Settings) => void;
  } = $props();

  // Edits stay in a draft until Save. A saved copy from the daemon replaces the draft.
  let settings = $state<Settings>({} as Settings);
  $effect.pre(() => {
    settings = { ...saved };
  });
  const dirty = $derived(JSON.stringify(settings) !== JSON.stringify(saved));
  const change = (patch: Partial<Settings>) => {
    settings = { ...settings, ...patch };
  };
  const discard = () => {
    settings = { ...saved };
  };

  const levels = ['off', 'lite', 'full', 'ultra'];
  const percent = (n: number) => `${n}%`;
  const tokens = (n: number) => (n === 0 ? 'auto' : `${n.toLocaleString()} tokens`);
  const count = (n: number) => (n === 0 ? 'off' : String(n));

  // Title model list: the free OpenRouter model is always offered first.
  const titleModels: Model[] = $derived([
    { id: 'openrouter/openrouter/free', label: 'OpenRouter Free', provider: 'openrouter', source: 'pi', kind: 'chat', efforts: [], image_input: false, context_window: 0, stale: false, updated_at: 0 },
    ...models.filter((model) => model.source === 'pi' && model.id !== 'openrouter/openrouter/free')
  ]);

  const subagents = [
    { key: 'spec_reviewer', name: 'Spec reviewer', description: 'Checks ticket requirements, missing work and scope creep.' },
    { key: 'standards_reviewer', name: 'Standards reviewer', description: 'Checks repository standards and code smells.' }
  ] as const;

  // Tool keys live in the runtime env file the tool shells read on every call, so a saved key
  // applies to the next call with no restart. The API never returns a stored value, only whether one is set.
  const labels: Record<string, string> = { brave: 'Brave search', exa: 'Exa search', jina: 'Jina reader' };
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
</script>

<div class="h-full overflow-y-auto px-4 py-6" in:rise|global>
  <div class="mx-auto flex max-w-[760px] flex-col gap-8">
    <header class="flex flex-col gap-1">
      <h2 class="text-[1.75rem] font-semibold leading-[1.2] tracking-[-0.02em]">Settings</h2>
      <p class="text-[13px] text-fg-3">
        Daemon-wide. Changes apply to the next Session, not to running ones.
        {#if saving}<span> Saving…</span>{/if}
      </p>
    </header>

    <div class="sticky top-0 z-10 flex items-center justify-between gap-3 rounded-2xl border border-border bg-surface-1 p-3">
      <span class="text-[13px] text-fg-3">{dirty ? 'Unsaved changes' : 'All changes saved'}</span>
      <div class="flex gap-2">
        <Button disabled={!dirty || saving} onclick={discard}>Discard</Button>
        <Button disabled={!dirty || saving} onclick={() => onsave(settings)}>Save</Button>
      </div>
    </div>

    <section class="flex flex-col gap-3" aria-labelledby="g-new">
      <h3 id="g-new" class="text-[1.0625rem] font-semibold">New Sessions</h3>
      <div class="flex flex-col gap-4 rounded-2xl border border-border bg-surface-1 p-4">
        <ModelPicker label="Default model" {models} value={settings.default_model}
          onchange={(default_model) => change({ default_model })} />
        <div class="flex flex-wrap items-center justify-between gap-4">
          <div>
            <div class="text-[13px]">Ponytail level</div>
            <div class="text-[12px] text-fg-3">Starting level for new Sessions.</div>
          </div>
          <div class="seg" role="group" aria-label="Ponytail level">
            {#each levels as level (level)}
              <button
                aria-pressed={settings.ponytail_default_level === level}
                onclick={() => change({ ponytail_default_level: level })}
              >{level}</button>
            {/each}
          </div>
        </div>
      </div>
    </section>

    <section class="flex flex-col gap-3" aria-labelledby="g-titles">
      <h3 id="g-titles" class="text-[1.0625rem] font-semibold">Session titles</h3>
      <div class="flex flex-col gap-4 rounded-2xl border border-border bg-surface-1 p-4">
        <div class="flex items-center justify-between gap-4">
          <div class="text-[13px]">Generate titles automatically</div>
          <Switch label="Generate titles automatically" checked={settings.title_generation}
            onchange={(title_generation) => change({ title_generation })} />
        </div>
        <ModelPicker label="Title model" models={titleModels} value={settings.title_model}
          onchange={(title_model) => change({ title_model })} />
        <p class="text-[12px] text-fg-3">Uses saved provider credentials. Retries until saved. Manual titles stay unchanged.</p>
      </div>
    </section>

    <section class="flex flex-col gap-3" aria-labelledby="g-context">
      <h3 id="g-context" class="text-[1.0625rem] font-semibold">Context</h3>
      <div class="flex flex-col gap-6 rounded-2xl border border-border bg-surface-1 p-4">
        <Slider label="Pressure band" min={1} max={100} step={1} format={percent}
          hint="Share of the context window where the model gets a nudge to compress, every turn."
          value={settings.max_context_limit} onchange={(max_context_limit) => change({ max_context_limit })} />
        <Slider label="Emergency band" min={1} max={100} step={1} format={percent}
          hint="Share of the context window where large tool output gets cut short."
          value={settings.emergency_threshold_percent}
          onchange={(emergency_threshold_percent) => change({ emergency_threshold_percent })} />
        <Slider label="Pressure floor" min={0} max={20000} step={500} format={tokens}
          hint="Smallest saving a fold must make. Auto picks the value itself."
          value={settings.min_pressure_benefit_tokens}
          onchange={(min_pressure_benefit_tokens) => change({ min_pressure_benefit_tokens })} />
        <Slider label="Preserved recent messages" min={0} max={50} step={1}
          hint="Newest messages that never fold."
          value={settings.preserve_recent_messages}
          onchange={(preserve_recent_messages) => change({ preserve_recent_messages })} />
        <Slider label="Tier 2 blocks" min={1} max={10000} step={100}
          value={settings.tier2_trigger} onchange={(tier2_trigger) => change({ tier2_trigger })} />
        <Slider label="Tier 3 blocks" min={1} max={10000} step={100}
          value={settings.tier3_trigger} onchange={(tier3_trigger) => change({ tier3_trigger })} />
      </div>
    </section>

    <section class="flex flex-col gap-3" aria-labelledby="g-tools">
      <h3 id="g-tools" class="text-[1.0625rem] font-semibold">Tools and proxy</h3>
      <div class="flex flex-col gap-4 rounded-2xl border border-border bg-surface-1 p-4">
        <div class="flex items-center justify-between gap-4">
          <div>
            <div class="text-[13px]">Billion context</div>
            <div class="text-[12px] text-fg-3">Compress, decompress and search the history through the proxy.</div>
          </div>
          <Switch label="Billion context" checked={settings.proxy_compression}
            onchange={(proxy_compression) => change({ proxy_compression })} />
        </div>
        <div class="flex items-center justify-between gap-4">
          <div>
            <div class="text-[13px]">Inject tools</div>
            <div class="text-[12px] text-fg-3">Declare compress, decompress, search and status to the model.</div>
          </div>
          <Switch label="Inject tools" checked={settings.inject_tool}
            onchange={(inject_tool) => change({ inject_tool })} />
        </div>
        <div class="flex items-center justify-between gap-4">
          <div>
            <div class="text-[13px]">Inject nudges</div>
            <div class="text-[12px] text-fg-3">Ask the model to compress when a band is crossed.</div>
          </div>
          <Switch label="Inject nudges" checked={settings.inject_nudge}
            onchange={(inject_nudge) => change({ inject_nudge })} />
        </div>
        <label class="flex flex-col gap-1 text-[13px]">
          Protected tools
          <input class="field" value={settings.protected_tools.join(', ')} placeholder="comma separated, e.g. read, bash"
            onchange={(event) => change({
              protected_tools: (event.currentTarget as HTMLInputElement).value
                .split(',').map((name) => name.trim()).filter(Boolean)
            })} />
          <span class="text-[12px] text-fg-3">Their results never fold.</span>
        </label>
      </div>
    </section>

    <section class="flex flex-col gap-3" aria-labelledby="g-todo">
      <h3 id="g-todo" class="text-[1.0625rem] font-semibold">Todo</h3>
      <div class="flex flex-col gap-4 rounded-2xl border border-border bg-surface-1 p-4">
        <div class="flex items-center justify-between gap-4">
          <div>
            <div class="text-[13px]">Stop reminders</div>
            <div class="text-[12px] text-fg-3">Wake the model when a turn ends with items still open.</div>
          </div>
          <Switch label="Stop reminders" checked={settings.todo_reminders}
            onchange={(todo_reminders) => change({ todo_reminders })} />
        </div>
        <Slider label="Reminders per turn" min={0} max={10} step={1} format={count}
          hint="Resets with the next message. Off stops reminders."
          value={settings.todo_reminders_max} onchange={(todo_reminders_max) => change({ todo_reminders_max })} />
        <div class="flex items-center justify-between gap-4">
          <div>
            <div class="text-[13px]">Open the plan on create</div>
            <div class="text-[12px] text-fg-3">The card expands for two seconds when the list changes.</div>
          </div>
          <Switch label="Open the plan on create" checked={settings.todo_auto_expand}
            onchange={(todo_auto_expand) => change({ todo_auto_expand })} />
        </div>
        <div class="flex items-center justify-between gap-4">
          <div>
            <div class="text-[13px]">Fade when done</div>
            <div class="text-[12px] text-fg-3">The card fades a few seconds after the last item resolves.</div>
          </div>
          <Switch label="Fade when done" checked={settings.todo_hide_when_done}
            onchange={(todo_hide_when_done) => change({ todo_hide_when_done })} />
        </div>
      </div>
    </section>

    <section class="flex flex-col gap-3" aria-labelledby="g-web">
      <h3 id="g-web" class="text-[1.0625rem] font-semibold">Web search and fetching</h3>
      <div class="flex flex-col gap-4 rounded-2xl border border-border bg-surface-1 p-4">
        <p class="text-[12px] text-fg-3">
          API keys for search and page reading. Saved to the tool keys file; the tools read it on their next call. An empty value removes a key.
        </p>
        {#each toolKeys as key (key.name)}
          <div class="flex items-end gap-2">
            <label class="flex min-w-0 flex-1 flex-col gap-1 text-[12px] text-fg-3">
              {labels[key.name] ?? key.name} <span class="font-mono text-fg-4">{key.variable}</span>
              ({key.set ? 'set' : 'not set'})
              <input class="field" type="password" autocomplete="off"
                placeholder={key.set ? 'Replace saved key' : 'Paste key'} bind:value={drafts[key.name]} />
            </label>
            <Button disabled={toolSaving === key.name || !(drafts[key.name] ?? '').trim()}
              onclick={() => void saveToolKey(key.name)}>Save</Button>
          </div>
        {/each}
        {#if toolError}<p class="text-[13px] text-status-failed">{toolError}</p>{/if}
      </div>
    </section>

    <Prompts />

    <section class="flex flex-col gap-3" aria-labelledby="g-subagents">
      <h3 id="g-subagents" class="text-[1.0625rem] font-semibold">Sub-agents</h3>
      <p class="text-[13px] text-fg-3">Defaults apply when a reviewer Session is created. Existing reviewers keep their model and reasoning effort.</p>
      <div class="grid gap-3 sm:grid-cols-2">
        {#each subagents as agent (agent.key)}
          {@const defaults = settings[agent.key]}
          {@const efforts = models.find((model) => model.id === defaults.model)?.efforts ?? []}
          <div class="flex min-w-0 flex-col gap-3 rounded-2xl border border-border bg-surface-1 p-4">
            <div>
              <h4 class="font-semibold">{agent.name}</h4>
              <p class="text-[13px] text-fg-3">{agent.description}</p>
            </div>
            <ModelPicker {models} value={defaults.model} label={`${agent.name} model`}
              onchange={(model) => change({ [agent.key]: { model, reasoning_effort: null } })} />
            {#if efforts.length}
              <span class="text-[12px] text-fg-3">Reasoning effort</span>
              <div class="seg flex-wrap" role="group" aria-label={`${agent.name} reasoning effort`}>
                {#each [null, ...efforts] as option (option)}
                  <button type="button" aria-pressed={defaults.reasoning_effort === option}
                    onclick={() => change({ [agent.key]: { ...defaults, reasoning_effort: option } })}>{option ?? 'Default'}</button>
                {/each}
              </div>
            {:else}
              <p class="text-[12px] text-fg-3">This model has no configurable reasoning effort.</p>
            {/if}
            <p class="text-[12px] text-fg-3">The edit tool is unavailable. Shell and other tools remain available; this is not a sandbox.</p>
          </div>
        {/each}
      </div>
    </section>
  </div>
</div>
