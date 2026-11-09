<script lang="ts">
  import { Slider } from 'bits-ui';

  // onchange fires once per release, not per drag step, so each drag saves one PUT.
  let {
    value,
    min,
    max,
    step = 1,
    label,
    hint = '',
    format = (n: number) => String(n),
    onchange
  }: {
    value: number;
    min: number;
    max: number;
    step?: number;
    label: string;
    hint?: string;
    format?: (n: number) => string;
    onchange: (value: number) => void;
  } = $props();

  let dragging = $state<number | null>(null);
  const shown = $derived(dragging ?? value);
</script>

<div class="flex flex-col gap-2">
  <div class="flex items-baseline justify-between gap-4 text-[13px]">
    <span>{label}</span>
    <span class="font-mono text-fg-2">{format(shown)}</span>
  </div>
  {#if hint}<p class="text-[12px] text-fg-3">{hint}</p>{/if}
  <Slider.Root
    type="single"
    {min}
    {max}
    {step}
    value={shown}
    onValueChange={(next) => (dragging = next)}
    onValueCommit={(next) => {
      dragging = null;
      onchange(next);
    }}
    class="relative flex h-5 w-full touch-none select-none items-center"
  >
    <span class="relative h-1.5 w-full grow rounded-full bg-surface-3">
      <Slider.Range class="absolute h-full rounded-full bg-brand-strong" />
    </span>
    <Slider.Thumb
      index={0}
      aria-label={label}
      class="block h-4 w-4 rounded-full border border-border bg-fg-1 shadow focus-visible:outline-2 focus-visible:outline-brand"
    />
  </Slider.Root>
</div>
