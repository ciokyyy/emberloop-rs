<!-- The turn reports itself in its own line above the composer card: activity glyph, the running
     tool in a few characters, and how long the turn has been going. Ported from the Go app. -->
<script lang="ts">
  import { shortTool } from '$lib/toolfeed';

  // tool is the name of the running tool, '' when none is known.
  let { status, tool = '', class: cls = '' }: { status: string; tool?: string; class?: string } = $props();

  const running = $derived(status === 'working');
  const short = $derived(shortTool(tool));

  // The count starts when the run does and stops with it, so a stale number never sits under an
  // idle composer.
  let startedAt = $state(0);
  let now = $state(0);
  $effect(() => {
    if (!running) {
      startedAt = 0;
      now = 0;
      return;
    }
    if (!startedAt) {
      startedAt = Date.now();
      now = startedAt;
    }
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });

  function elapsedText(ms: number) {
    const total = Math.max(0, Math.round(ms / 1000));
    if (total < 60) return `${total}s`;
    const minutes = Math.floor(total / 60);
    const seconds = total % 60;
    return seconds ? `${minutes}m ${seconds}s` : `${minutes}m`;
  }
</script>

{#if running}
  <div
    data-testid="working-indicator"
    role="status"
    class="flex min-w-0 items-center gap-1.5 px-1 text-[12px] text-status-working {cls}"
    title={short ? `Working: ${short}` : 'Working'}
  >
    <svg
      class="working-spinner size-3.5 shrink-0 animate-spin motion-reduce:animate-none"
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" stroke="currentColor" stroke-opacity="0.25" stroke-width="3" />
      <path d="M21 12a9 9 0 0 0-9-9" stroke="currentColor" stroke-width="3" stroke-linecap="round" />
    </svg>
    <span class="sr-only">Working</span>
    {#if short}
      <span class="truncate font-mono text-fg-2">{short}</span>
    {:else}
      <span class="truncate text-fg-3">Working</span>
    {/if}
    {#if startedAt}
      <span class="shrink-0 tabular-nums text-fg-4">· {elapsedText(now - startedAt)}</span>
    {/if}
  </div>
{/if}
