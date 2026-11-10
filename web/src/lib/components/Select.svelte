<script lang="ts">
  import type { Snippet } from 'svelte';
  import { Select as SelectPrimitive } from 'bits-ui';
  import CheckIcon from '@lucide/svelte/icons/check';
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import { slideDown } from '$lib/motion';
  import { cn } from '$lib/utils';

  // The one select. Controlled: `value` in, `onchange` out. The label comes from `value`,
  // not the menu's own state, so an action item such as "+ Add project" never shows as the value.
  // `trigger` replaces the trigger's contents when a caller needs more than one line of label.
  let { value = '', options, label, placeholder = '', disabled = false, class: className, onchange, trigger }: {
    value?: string;
    options: { value: string; label: string }[];
    label: string;
    placeholder?: string;
    disabled?: boolean;
    class?: string;
    onchange?: (value: string) => void;
    trigger?: Snippet<[string]>;
  } = $props();

  let open = $state(false);
  const shown = $derived(options.find((option) => option.value === value)?.label ?? placeholder);
</script>

<SelectPrimitive.Root type="single" {value} bind:open {disabled} onValueChange={(next) => onchange?.(next)}>
  <SelectPrimitive.Trigger aria-label={label} class={cn('field flex items-center justify-between gap-2 text-left cursor-pointer disabled:cursor-not-allowed disabled:opacity-50 hover:border-fg-3 focus-visible:outline-none focus-visible:border-fg-3', className)}>
    {#if trigger}
      {@render trigger(shown)}
    {:else}
      <span class="truncate">{shown}</span>
      <ChevronDownIcon class={cn('size-4 shrink-0 text-fg-3 transition-transform', open && 'rotate-180')} />
    {/if}
  </SelectPrimitive.Trigger>
  <SelectPrimitive.Portal>
    <SelectPrimitive.Content forceMount sideOffset={6}>
      {#snippet child({ props, wrapperProps })}
        {#if open}
          <div {...wrapperProps} class="z-[70]">
            <div {...props} transition:slideDown class="z-[70] max-h-[min(45dvh,360px)] min-w-(--bits-select-anchor-width) overflow-y-auto rounded-xl border border-border bg-surface-2 p-1 shadow-xl">
              {#each options as option (option.value)}
                <SelectPrimitive.Item value={option.value} label={option.label}
                  class="relative flex w-full cursor-pointer select-none items-center rounded-lg py-2 pr-8 pl-2 text-[13px] text-fg-1 outline-none transition-colors data-highlighted:bg-surface-3">
                  {#snippet children({ selected })}
                    {option.label}
                    {#if selected}<CheckIcon class="absolute right-2 size-4 text-fg-2" />{/if}
                  {/snippet}
                </SelectPrimitive.Item>
              {/each}
            </div>
          </div>
        {/if}
      {/snippet}
    </SelectPrimitive.Content>
  </SelectPrimitive.Portal>
</SelectPrimitive.Root>
