<!-- A harness retry banner is a status line, not prose: one dot and one truncated line. -->
<script lang="ts">
  // The row's own fields, so a leaf status line does not import the feed reducer model.
  let { text, retry }: { text?: string; retry?: 'retrying' | 'finished' } = $props();
</script>

<div
  class="my-1 flex min-w-0 items-center gap-2 px-1 text-[13px] text-fg-3"
  data-testid="retry-row"
  data-state={retry}
  role="status"
>
  <span
    class="h-[5px] w-[5px] shrink-0 rounded-full {retry === 'retrying' ? 'bg-status-failed' : 'bg-fg-4'}"
    aria-hidden="true"
  ></span>
  <span class="min-w-0 truncate">{text}</span>
</div>
