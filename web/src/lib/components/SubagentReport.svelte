<script lang="ts">
  import type { ReviewerReport, SteerRoute } from '$lib/api/types';
  import { renderMarkdown } from '$lib/markdown';
  import { sessionPath } from '$lib/route';

  let { report, route, projectId }: { report: ReviewerReport; route?: SteerRoute; projectId: string } = $props();
  const failed = $derived(report.status === 'error' || report.status === 'stopped');
  const ROLE_LABELS: Record<string, string> = {
    spec: 'Spec reviewer',
    standards: 'Standards reviewer',
    worker: 'Worker',
    scout: 'Scout'
  };
  const roleLabel = $derived(ROLE_LABELS[report.role] ?? report.role);
</script>

<section aria-label={failed ? 'Sub-agent error' : 'Sub-agent report'}
  class="my-2 min-w-0 rounded-xl border bg-surface-1 px-4 py-3 text-[13px] {failed ? 'border-status-failed' : 'border-border'}">
  <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
    <span class="font-semibold">{failed ? 'Sub-agent error' : 'Sub-agent report'}</span>
    <a class="text-brand underline underline-offset-2" href={sessionPath(projectId, report.sessionId)}>
      {roleLabel}
    </a>
    <span class="chip">{report.status}</span>
    <span class="ml-auto text-[12px] text-fg-3">{route === 'pending' ? 'Queued' : 'Delivered'}</span>
  </div>
  {#if report.ticketPath}
    <div class="mt-1 break-all font-mono text-[12px] text-fg-3">{report.ticketPath}</div>
  {/if}
  {#if report.summary}<p class="mt-2 break-words">{report.summary}</p>{/if}
  {#if report.report}
    <details class="mt-2">
      <summary class="cursor-pointer text-fg-3">Full report</summary>
      <div class="md mt-2 min-w-0">{@html renderMarkdown(report.report)}</div>
    </details>
  {/if}
</section>
