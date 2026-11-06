<script lang="ts">
  import type { Model } from '$lib/api/types';

  let {
    models = [],
    value = $bindable(''),
    label = 'Model',
    disabled = false,
    onchange
  }: {
    models?: Model[];
    value?: string;
    label?: string;
    disabled?: boolean;
    onchange?: (id: string) => void;
  } = $props();

  function windowLabel(model: Model) {
    return model.context_window >= 1_000_000
      ? `${Math.round(model.context_window / 1_000_000)}M`
      : `${Math.round(model.context_window / 1000)}k`;
  }

  function change(event: Event) {
    value = (event.currentTarget as HTMLSelectElement).value;
    onchange?.(value);
  }
</script>

<label class="flex flex-col gap-1.5 text-[12px] text-fg-3">
  {label}
  <select class="field" {disabled} {value} onchange={change}>
    <option value="">agy default</option>
    {#each models as model (model.id)}
      <option value={model.id}>
        {model.label} · {windowLabel(model)}{model.stale ? ' · stale' : ''}
      </option>
    {/each}
  </select>
</label>
