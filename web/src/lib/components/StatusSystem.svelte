<script lang="ts">
  import type { SystemStatus } from '$lib/api/types';
  import { formatBytes, formatUptime, meterTextTone, ringStrokeColor } from '$lib/status';

  let {
    data,
    loading = false,
    live = true,
    ontogglelive,
    onrefresh
  }: {
    data: SystemStatus;
    loading?: boolean;
    live?: boolean;
    ontogglelive: () => void;
    onrefresh: () => void;
  } = $props();

  const R = 22;
  const C = 2 * Math.PI * R;

  const cards = $derived([
    {
      label: 'CPU',
      percent: Math.min(100, Math.max(0, data.cpu.percent)),
      primary: `${data.cpu.percent.toFixed(1)}%`,
      detail: `${data.cpu.cores} cores · load ${data.cpu.loadAvg1.toFixed(2)}`
    },
    {
      label: 'Memory',
      percent: Math.min(100, Math.max(0, data.memory.percent)),
      primary: `${formatBytes(data.memory.usedBytes)} / ${formatBytes(data.memory.totalBytes)}`,
      detail: `${formatBytes(data.memory.availableBytes)} avail · ${formatBytes(data.memory.cachedBytes)} cache`
    },
    {
      label: 'Disk space',
      percent: Math.min(100, Math.max(0, data.disk.percent)),
      primary: `${formatBytes(data.disk.usedBytes)} / ${formatBytes(data.disk.totalBytes)}`,
      detail: `${formatBytes(data.disk.availableBytes)} free`
    }
  ]);

  const segmentColor: Record<string, string> = {
    accent: '#88BCFF',
    needs: '#E2B623',
    done: '#90CA90',
    muted: '#9E9E9E'
  };

  function stateBadgeClass(state: string): string {
    const s = state.toLowerCase();
    if (s === 'working' || s === 'live') return 'bg-[#1A2738] text-[#88BCFF] border-[#29405E]';
    if (s === 'running' || s === 'healthy') return 'bg-[#1D2E1D] text-[#90CA90] border-[#2E4A2E]';
    return 'bg-surface-3 text-fg-3 border-border';
  }
</script>

<div class="flex flex-col gap-3">
  <div class="flex flex-wrap items-center justify-between gap-2">
    <span class="text-[12px] text-fg-3 tabular-nums">
      {data.host.hostname} · {data.host.os} · v{data.host.version} · up {formatUptime(data.host.uptimeSeconds)}
    </span>
    <div class="flex items-center gap-2">
      <button
        type="button"
        class="inline-flex h-8 items-center gap-1.5 rounded-lg border px-2.5 text-[12px] font-medium transition-colors {live
          ? 'border-[#2E4A2E] bg-[#1D2E1D] text-[#90CA90]'
          : 'border-border bg-surface-2 text-fg-3 hover:text-fg-1'}"
        aria-pressed={live}
        onclick={ontogglelive}
      >
        <span class="h-1.5 w-1.5 rounded-full {live ? 'bg-[#90CA90]' : 'bg-fg-3'}"></span>
        {live ? 'Live · 5s' : 'Paused'}
      </button>
      <button
        type="button"
        class="btn h-8 shrink-0 px-3 text-[12px]"
        disabled={loading}
        onclick={onrefresh}
      >
        {loading ? 'Refreshing…' : 'Refresh'}
      </button>
    </div>
  </div>

  <div class="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
    {#each cards as card (card.label)}
      <div class="flex items-center gap-3.5 rounded-xl border border-border bg-surface-2 p-3.5">
        <div class="relative grid h-14 w-14 shrink-0 place-items-center">
          <svg viewBox="0 0 56 56" class="h-14 w-14 -rotate-90" aria-hidden="true">
            <circle
              cx="28"
              cy="28"
              r={R}
              fill="none"
              stroke="var(--surface-3)"
              stroke-width="4.5"
            />
            <circle
              cx="28"
              cy="28"
              r={R}
              fill="none"
              stroke={ringStrokeColor(card.percent)}
              stroke-width="4.5"
              stroke-linecap="round"
              stroke-dasharray={C}
              stroke-dashoffset={C * (1 - card.percent / 100)}
              style="transition: stroke-dashoffset 250ms ease"
            />
          </svg>
          <span class="absolute text-[11px] font-semibold tabular-nums {meterTextTone(card.percent)}">
            {Math.round(card.percent)}%
          </span>
        </div>
        <div class="min-w-0 flex-1">
          <div class="text-[13px] font-semibold text-fg-1">{card.label}</div>
          <div class="truncate text-[12px] font-medium text-fg-2 tabular-nums">{card.primary}</div>
          <div class="truncate text-[11px] text-fg-3 tabular-nums">{card.detail}</div>
        </div>
      </div>
    {/each}
  </div>

  <div class="overflow-hidden rounded-xl border border-border bg-surface-2">
    <div class="flex flex-wrap items-center justify-between gap-2 border-b border-border px-3.5 py-2.5">
      <h2 class="text-[13px] font-semibold text-fg-1">Processes & runtime</h2>
      <span class="text-[12px] text-fg-3 tabular-nums">
        {data.runtime.activeSessions} active · {data.runtime.workingSessions} working · {data.runtime.archivedSessions} archived
      </span>
    </div>
    <div class="overflow-x-auto">
      <table class="w-full border-collapse text-left">
        <thead>
          <tr class="border-b border-border bg-[#1F1F1F] text-[11px] font-medium text-fg-3">
            <th class="px-3.5 py-2">Process</th>
            <th class="px-3 py-2">Detail</th>
            <th class="px-3 py-2 text-right">CPU</th>
            <th class="px-3 py-2 text-right">RAM</th>
            <th class="px-3.5 py-2 text-right">State</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-border/70 text-[12px]">
          {#each data.processes as proc (proc.pid > 0 ? `${proc.pid}-${proc.name}` : proc.name)}
            <tr class="hover:bg-surface-3/40">
              <td class="px-3.5 py-2 font-medium text-fg-1">
                {proc.name}
                {#if proc.pid > 0}
                  <span class="ml-1.5 text-[11px] font-normal text-fg-3 tabular-nums">#{proc.pid}</span>
                {/if}
              </td>
              <td class="max-w-[260px] truncate px-3 py-2 text-fg-3">{proc.detail}</td>
              <td class="px-3 py-2 text-right text-fg-2 tabular-nums">{proc.cpuPercent.toFixed(1)}%</td>
              <td class="px-3 py-2 text-right text-fg-2 tabular-nums">{formatBytes(proc.rssBytes)}</td>
              <td class="px-3.5 py-2 text-right">
                <span class="inline-flex items-center rounded-full border px-2 py-0.5 text-[11px] font-medium capitalize {stateBadgeClass(proc.state)}">
                  {proc.state}
                </span>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </div>

  <div class="flex flex-col gap-2.5 rounded-xl border border-border bg-surface-2 p-3.5">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <span class="truncate text-[12px] font-medium text-fg-1" title={data.storage.rootPath}>
        Data root · <span class="font-normal text-fg-3">{data.storage.rootPath}</span>
      </span>
      <span class="text-[12px] text-fg-2 tabular-nums">
        {formatBytes(data.storage.totalUsedBytes)} · {data.storage.diskPercent.toFixed(1)}% of {formatBytes(data.storage.diskTotalBytes)}
      </span>
    </div>
    <div class="flex h-2 w-full overflow-hidden rounded-full bg-surface-3">
      {#each data.storage.segments as seg (seg.id)}
        {#if seg.percent > 0}
          <div
            class="h-full"
            style:width="{Math.max(1.5, seg.percent)}%"
            style:background={segmentColor[seg.tone] ?? '#9E9E9E'}
            title="{seg.label}: {formatBytes(seg.bytes)} ({seg.percent.toFixed(1)}%)"
          ></div>
        {/if}
      {/each}
    </div>
    <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
      {#each data.storage.segments as seg (seg.id)}
        <div class="flex items-center gap-2 text-[11px]">
          <span
            class="h-2 w-2 shrink-0 rounded-full"
            style:background={segmentColor[seg.tone] ?? '#9E9E9E'}
          ></span>
          <span class="truncate text-fg-3">{seg.label}</span>
          <span class="ml-auto font-medium text-fg-2 tabular-nums">{formatBytes(seg.bytes)}</span>
        </div>
      {/each}
    </div>
  </div>
</div>
