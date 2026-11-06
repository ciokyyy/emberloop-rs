<script lang="ts">
  import type { Snippet } from 'svelte';
  import { cn } from '$lib/utils';

  let {
    variant = 'default',
    class: className = '',
    type = 'button',
    disabled = false,
    title,
    'aria-label': ariaLabel,
    onclick,
    children
  }: {
    variant?: 'default' | 'primary';
    class?: string;
    type?: 'button' | 'submit';
    disabled?: boolean;
    title?: string;
    'aria-label'?: string;
    onclick?: (event: MouseEvent) => void;
    children?: Snippet;
  } = $props();
</script>

<button
  {type}
  {disabled}
  {title}
  aria-label={ariaLabel}
  {onclick}
  class={cn('btn', variant === 'primary' && 'btn-pri', className)}
>
  {@render children?.()}
</button>
