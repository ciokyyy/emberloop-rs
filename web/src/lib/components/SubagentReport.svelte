<script lang="ts">
  import type { ReviewerReport, SteerRoute } from '$lib/api/types';
  import { renderMarkdown } from '$lib/markdown';
  import { sessionPath } from '$lib/route';

  let { report, route, projectId }: { report: ReviewerReport; route?: SteerRoute; projectId: string } = $props();
  const failed = $derived(report.status === 'error' || report.status === 'stopped');
</script>

<section aria-label={failed ? 'Sub-agent error' : 'Sub-agent report'}
  class="my-2 min-w-0 rounded-xl border bg-surface-1 px-4 py-3 text-[13px] {failed ? 'border-status-failed' : 'border-border'}">
  <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
    <span class="font-semibold">{failed ? 'Sub-agent error' : 'Sub-agent report'}</span>
    <a class="text-brand underline underline-offset-2" href={sessionPath(projectId, report.sessionId)}>
      {report.role === 'spec' ? 'Spec reviewer' : 'Standards reviewer'}
    </a>
    <span class="chip">{report.status}</span>
    <span class="ml-auto text-[12px] text-fg-3">{route === 'pending' ? 'Queued' : 'Delivered'}</span>
  </div>
  <div class="mt-1 break-all font-mono text-[12px] text-fg-3">{report.ticketPath}</div>
  {#if report.summary}<p class="mt-2 break-words">{report.summary}</p>{/if}
  {#if report.report}
    <details class="mt-2">
      <summary class="cursor-pointer text-fg-3">Full report</summary>
      <div class="prose mt-2 min-w-0 break-words">{@html renderMarkdown(report.report)}</div>
    </details>
  {/if}
</section>
