<script lang="ts">
  import {
    createPaginatedRowModel,
    createSortedRowModel,
    createTable,
    createTableState,
    rowPaginationFeature,
    rowSortingFeature,
    sortFns,
    tableFeatures,
    type ColumnDef,
    type PaginationState,
    type SortingState
  } from '@tanstack/svelte-table';
  import type { TokenStatus } from '$lib/api/types';
  import { modelIconId } from '$lib/brands';
  import Brand from './Brand.svelte';
  import {
    DEFAULT_TOKEN_FILTERS,
    filterTokenRows,
    formatCompactTokens,
    formatLatency,
    formatMonthLabel,
    groupTokenRows,
    rowsToCsv,
    summarizeTokenRows,
    uniqueHarnesses,
    uniqueModels,
    type GroupMode,
    type GroupedTokenRow,
    type TokenFilterState
  } from '$lib/status';

  let {
    data,
    loading = false,
    onmonthchange,
    onrefresh
  }: {
    data: TokenStatus;
    loading?: boolean;
    onmonthchange: (month: string) => void;
    onrefresh: () => void;
  } = $props();

  let filters = $state<TokenFilterState>({ ...DEFAULT_TOKEN_FILTERS });
  let groupMode = $state<GroupMode>('model-harness');

  const harnesses = $derived(uniqueHarnesses(data.rows));
  const models = $derived(uniqueModels(data.rows));
  const filteredRows = $derived(filterTokenRows(data.rows, filters));
  const groupedRows = $derived(groupTokenRows(filteredRows, groupMode));
  const totals = $derived(summarizeTokenRows(groupedRows));
  const maxRowTokens = $derived(
    groupedRows.reduce((max, row) => (row.totalTokens > max ? row.totalTokens : max), 0)
  );

  const hasActiveFilters = $derived(
    filters.search.trim() !== '' ||
      filters.harness !== 'all' ||
      filters.model !== 'all' ||
      filters.minLatencyMs > 0 ||
      filters.minCalls > 0
  );

  const features = tableFeatures({
    rowSortingFeature,
    rowPaginationFeature,
    sortedRowModel: createSortedRowModel(),
    paginatedRowModel: createPaginatedRowModel(),
    sortFns
  });

  const [sorting, setSorting] = createTableState<SortingState>([
    { id: 'totalTokens', desc: true }
  ]);
  const [pagination, setPagination] = createTableState<PaginationState>({
    pageIndex: 0,
    pageSize: 12
  });

  const columns: ColumnDef<typeof features, GroupedTokenRow>[] = [
    { id: 'primary', accessorFn: (r) => r.primary, header: 'Model', sortFn: 'alphanumeric' },
    {
      id: 'harnessLabel',
      accessorFn: (r) => r.harnessLabel,
      header: 'Harness',
      sortFn: 'alphanumeric'
    },
    { id: 'calls', accessorFn: (r) => r.calls, header: 'Calls' },
    { id: 'inputTokens', accessorFn: (r) => r.inputTokens, header: 'Input' },
    { id: 'outputTokens', accessorFn: (r) => r.outputTokens, header: 'Output' },
    { id: 'cachedTokens', accessorFn: (r) => r.cachedTokens, header: 'Cached' },
    { id: 'avgLatencyMs', accessorFn: (r) => r.avgLatencyMs, header: 'Time / call' },
    { id: 'totalTokens', accessorFn: (r) => r.totalTokens, header: 'Total' }
  ];

  const table = createTable({
    features,
    columns,
    get data() {
      return groupedRows;
    },
    state: {
      get sorting() {
        return sorting();
      },
      get pagination() {
        return pagination();
      }
    },
    onSortingChange: setSorting,
    onPaginationChange: setPagination
  });

  const visibleRows = $derived(table.getRowModel().rows);
  const currentPage = $derived(pagination().pageIndex + 1);
  const pageCount = $derived(Math.max(1, table.getPageCount()));

  function resetPage() {
    setPagination((prev) => (prev.pageIndex === 0 ? prev : { ...prev, pageIndex: 0 }));
  }

  function resetFilters() {
    filters = { ...DEFAULT_TOKEN_FILTERS };
    resetPage();
  }

  function harnessDot(harness: string): string {
    if (harness === 'claude') return '#d6a87b';
    if (harness === 'pi') return '#8b7cf6';
    if (harness === 'agy') return '#5b9ac6';
    return '#9e9e9e';
  }

  function exportCsv() {
    const csv = rowsToCsv(groupedRows);
    const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = `tokens-${data.month}.csv`;
    link.click();
    URL.revokeObjectURL(url);
  }

  function primaryColumnLabel(mode: GroupMode): string {
    if (mode === 'harness') return 'Harness';
    if (mode === 'project') return 'Project';
    return 'Model';
  }
</script>

<div class="flex flex-col gap-3">
  <div class="flex flex-wrap items-center justify-between gap-2">
    <div class="inline-flex items-center rounded-lg border border-border bg-surface-2">
      <button
        type="button"
        class="grid h-8 w-8 place-items-center text-fg-2 hover:text-fg-1 disabled:opacity-40"
        aria-label="Previous month"
        disabled={loading || !data.prevMonth}
        onclick={() => {
          if (data.prevMonth) {
            resetPage();
            onmonthchange(data.prevMonth);
          }
        }}
      >
        ‹
      </button>
      {#if data.availableMonths.length > 1}
        <select
          aria-label="Month"
          class="h-8 bg-transparent px-1 text-[12px] font-medium text-fg-1 focus:outline-none"
          value={data.month}
          disabled={loading}
          onchange={(e) => {
            resetPage();
            onmonthchange(e.currentTarget.value);
          }}
        >
          {#each data.availableMonths as month (month)}
            <option value={month} class="bg-surface-2 text-fg-1">{formatMonthLabel(month)}</option>
          {/each}
        </select>
      {:else}
        <span class="px-2 text-[12px] font-medium text-fg-1">{formatMonthLabel(data.month)}</span>
      {/if}
      <button
        type="button"
        class="grid h-8 w-8 place-items-center text-fg-2 hover:text-fg-1 disabled:opacity-40"
        aria-label="Next month"
        disabled={loading || !data.nextMonth}
        onclick={() => {
          if (data.nextMonth) {
            resetPage();
            onmonthchange(data.nextMonth);
          }
        }}
      >
        ›
      </button>
    </div>

    <div class="flex items-center gap-2">
      <button
        type="button"
        class="btn h-8 shrink-0 px-3 text-[12px]"
        disabled={groupedRows.length === 0}
        onclick={exportCsv}
      >
        Export CSV
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

  <div class="grid grid-cols-2 gap-2.5 lg:grid-cols-4">
    <div class="rounded-xl border border-border bg-surface-2 p-3.5">
      <div class="text-[11px] font-medium text-fg-3">Total tokens</div>
      <div class="mt-1 text-[18px] font-semibold tracking-tight text-fg-1 tabular-nums">
        {formatCompactTokens(data.summary.totalTokens)}
      </div>
      <div class="mt-0.5 text-[11px] text-fg-3 tabular-nums">
        {data.summary.totalCalls.toLocaleString()} calls
      </div>
    </div>

    <div class="rounded-xl border border-border bg-surface-2 p-3.5">
      <div class="text-[11px] font-medium text-fg-3">Cache hit</div>
      <div class="mt-1 text-[18px] font-semibold tracking-tight text-[#90CA90] tabular-nums">
        {data.summary.cacheHitPercent.toFixed(1)}%
      </div>
      <div class="mt-0.5 text-[11px] text-fg-3 tabular-nums">
        {formatCompactTokens(data.summary.cachedTokens)} cached read
      </div>
    </div>

    <div class="rounded-xl border border-border bg-surface-2 p-3.5">
      <div class="text-[11px] font-medium text-fg-3">Avg latency</div>
      <div class="mt-1 text-[18px] font-semibold tracking-tight text-[#88BCFF] tabular-nums">
        {formatLatency(data.summary.avgLatencyMs)}
      </div>
      <div class="mt-0.5 text-[11px] text-fg-3 tabular-nums">
        p95 {formatLatency(data.summary.p95LatencyMs)}
      </div>
    </div>

    <div class="rounded-xl border border-border bg-surface-2 p-3.5">
      <div class="text-[11px] font-medium text-fg-3">Top token sink</div>
      <div class="mt-1 truncate text-[16px] font-semibold tracking-tight text-fg-1">
        {data.summary.topSinkName || '–'}
      </div>
      <div class="mt-0.5 text-[11px] text-fg-3 tabular-nums">
        {#if data.summary.topSinkTokens > 0}
          {formatCompactTokens(data.summary.topSinkTokens)} ({data.summary.topSinkSharePercent.toFixed(0)}%)
        {:else}
          No activity yet
        {/if}
      </div>
    </div>
  </div>

  <div class="flex flex-col gap-2 rounded-xl border border-border bg-surface-2 p-3">
    <div class="flex flex-wrap items-center gap-2">
      <input
        type="search"
        aria-label="Filter telemetry"
        placeholder="Filter model, harness, project…"
        class="h-8 min-w-[180px] flex-1 rounded-lg border border-border bg-surface-1 px-2.5 text-[12px] text-fg-1 placeholder:text-fg-3 focus:border-fg-3 focus:outline-none"
        value={filters.search}
        oninput={(e) => {
          filters.search = e.currentTarget.value;
          resetPage();
        }}
      />

      <select
        aria-label="Harness filter"
        class="h-8 rounded-lg border border-border bg-surface-1 px-2 text-[12px] text-fg-2 focus:outline-none"
        value={filters.harness}
        onchange={(e) => {
          filters.harness = e.currentTarget.value;
          resetPage();
        }}
      >
        <option value="all">All harnesses</option>
        {#each harnesses as h (h.value)}
          <option value={h.value}>{h.label}</option>
        {/each}
      </select>

      <select
        aria-label="Model filter"
        class="h-8 max-w-[200px] rounded-lg border border-border bg-surface-1 px-2 text-[12px] text-fg-2 focus:outline-none"
        value={filters.model}
        onchange={(e) => {
          filters.model = e.currentTarget.value;
          resetPage();
        }}
      >
        <option value="all">All models</option>
        {#each models as model (model)}
          <option value={model}>{model}</option>
        {/each}
      </select>

      <select
        aria-label="Minimum latency"
        class="h-8 rounded-lg border border-border bg-surface-1 px-2 text-[12px] text-fg-2 focus:outline-none"
        value={String(filters.minLatencyMs)}
        onchange={(e) => {
          filters.minLatencyMs = Number(e.currentTarget.value);
          resetPage();
        }}
      >
        <option value="0">Any latency</option>
        <option value="1000">Latency ≥ 1s</option>
        <option value="3000">Latency ≥ 3s</option>
        <option value="8000">Latency ≥ 8s</option>
      </select>

      <select
        aria-label="Minimum calls"
        class="h-8 rounded-lg border border-border bg-surface-1 px-2 text-[12px] text-fg-2 focus:outline-none"
        value={String(filters.minCalls)}
        onchange={(e) => {
          filters.minCalls = Number(e.currentTarget.value);
          resetPage();
        }}
      >
        <option value="0">Any calls</option>
        <option value="10">Calls ≥ 10</option>
        <option value="50">Calls ≥ 50</option>
        <option value="100">Calls ≥ 100</option>
      </select>

      <select
        aria-label="Group by"
        class="h-8 rounded-lg border border-border bg-surface-1 px-2 text-[12px] text-fg-1 focus:outline-none"
        value={groupMode}
        onchange={(e) => {
          groupMode = e.currentTarget.value as GroupMode;
          resetPage();
        }}
      >
        <option value="model-harness">Group: Model + Harness</option>
        <option value="model">Group: Model</option>
        <option value="harness">Group: Harness</option>
        <option value="project">Group: Project</option>
      </select>
    </div>

    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="flex flex-wrap items-center gap-1.5">
        {#if filters.search.trim()}
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-full border border-border bg-surface-3 px-2 py-0.5 text-[11px] text-fg-2 hover:text-fg-1"
            onclick={() => {
              filters.search = '';
              resetPage();
            }}
          >
            search: {filters.search.trim()} ×
          </button>
        {/if}
        {#if filters.harness !== 'all'}
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-full border border-border bg-surface-3 px-2 py-0.5 text-[11px] text-fg-2 hover:text-fg-1"
            onclick={() => {
              filters.harness = 'all';
              resetPage();
            }}
          >
            harness: {filters.harness} ×
          </button>
        {/if}
        {#if filters.model !== 'all'}
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-full border border-border bg-surface-3 px-2 py-0.5 text-[11px] text-fg-2 hover:text-fg-1"
            onclick={() => {
              filters.model = 'all';
              resetPage();
            }}
          >
            model: {filters.model} ×
          </button>
        {/if}
        {#if filters.minLatencyMs > 0}
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-full border border-border bg-surface-3 px-2 py-0.5 text-[11px] text-fg-2 hover:text-fg-1"
            onclick={() => {
              filters.minLatencyMs = 0;
              resetPage();
            }}
          >
            latency ≥ {formatLatency(filters.minLatencyMs)} ×
          </button>
        {/if}
        {#if filters.minCalls > 0}
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-full border border-border bg-surface-3 px-2 py-0.5 text-[11px] text-fg-2 hover:text-fg-1"
            onclick={() => {
              filters.minCalls = 0;
              resetPage();
            }}
          >
            calls ≥ {filters.minCalls} ×
          </button>
        {/if}
        {#if hasActiveFilters}
          <button
            type="button"
            class="text-[11px] text-fg-3 underline hover:text-fg-1"
            onclick={resetFilters}
          >
            Reset
          </button>
        {/if}
      </div>
      <span class="text-[11px] text-fg-3 tabular-nums">
        Showing {groupedRows.length} {groupedRows.length === 1 ? 'group' : 'groups'} · {filteredRows.length} rows
      </span>
    </div>
  </div>

  <div class="overflow-hidden rounded-xl border border-border bg-surface-2">
    <div class="overflow-x-auto">
      <table class="w-full border-collapse text-left">
        <thead>
          {#each table.getHeaderGroups() as headerGroup (headerGroup.id)}
            <tr class="border-b border-border bg-[#1F1F1F] text-[11px] font-medium text-fg-3">
              {#each headerGroup.headers as header, idx (header.id)}
                {@const sorted = header.column.getIsSorted()}
                {@const alignRight = idx >= 2}
                <th class="px-3 py-2 {idx === 0 ? 'pl-3.5' : ''} {idx === 7 ? 'pr-3.5' : ''} {alignRight ? 'text-right' : ''}">
                  <button
                    type="button"
                    class="inline-flex items-center gap-1 hover:text-fg-1 {alignRight ? 'justify-end' : ''}"
                    onclick={header.column.getToggleSortingHandler()}
                  >
                    <span>
                      {header.id === 'primary'
                        ? primaryColumnLabel(groupMode)
                        : String(header.column.columnDef.header ?? '')}
                    </span>
                    {#if sorted === 'asc'}
                      <span aria-hidden="true">↑</span>
                    {:else if sorted === 'desc'}
                      <span aria-hidden="true">↓</span>
                    {/if}
                  </button>
                </th>
              {/each}
            </tr>
          {/each}
        </thead>

        <tbody class="divide-y divide-border/70 text-[12px]">
          {#if visibleRows.length === 0}
            <tr>
              <td colspan="8" class="px-4 py-8 text-center text-[13px] text-fg-3">
                {data.rows.length === 0
                  ? `No token usage recorded for ${formatMonthLabel(data.month)}.`
                  : 'No rows match active filters.'}
              </td>
            </tr>
          {:else}
            {#each visibleRows as row (row.original.id)}
              {@const item = row.original}
              {@const sharePct = maxRowTokens > 0 ? (item.totalTokens / maxRowTokens) * 100 : 0}
              <tr class="hover:bg-surface-3/40">
                <td class="py-2 pl-3.5 pr-3">
                  <div class="flex items-center gap-2">
                    <Brand
                      id={modelIconId({ provider: item.iconProvider, source: item.iconSource })}
                      size={16}
                    />
                    <div class="min-w-0">
                      <div class="truncate font-medium text-fg-1">{item.primary}</div>
                      <div class="truncate text-[11px] text-fg-3">{item.secondary}</div>
                    </div>
                  </div>
                </td>
                <td class="px-3 py-2">
                  <span class="inline-flex items-center gap-1.5 rounded-full border border-border bg-surface-3 px-2 py-0.5 text-[11px] text-fg-2">
                    <span
                      class="h-1.5 w-1.5 rounded-full"
                      style:background={harnessDot(item.harness)}
                    ></span>
                    {item.harnessLabel}
                  </span>
                </td>
                <td class="px-3 py-2 text-right text-fg-2 tabular-nums">
                  {item.calls.toLocaleString()}
                </td>
                <td class="px-3 py-2 text-right text-fg-2 tabular-nums">
                  {formatCompactTokens(item.inputTokens)}
                </td>
                <td class="px-3 py-2 text-right text-fg-2 tabular-nums">
                  {formatCompactTokens(item.outputTokens)}
                </td>
                <td class="px-3 py-2 text-right tabular-nums">
                  <span class="text-fg-2">{formatCompactTokens(item.cachedTokens)}</span>
                  <span class="ml-1 text-[11px] text-[#90CA90]">{item.cacheHitPercent.toFixed(0)}%</span>
                </td>
                <td class="px-3 py-2 text-right tabular-nums">
                  <div class="text-fg-1">{formatLatency(item.avgLatencyMs)}</div>
                  <div class="text-[10px] text-fg-3">p95 {formatLatency(item.p95LatencyMs)}</div>
                </td>
                <td class="py-2 pl-3 pr-3.5 text-right tabular-nums">
                  <div class="font-medium text-fg-1">{formatCompactTokens(item.totalTokens)}</div>
                  <div class="ml-auto mt-1 h-1 w-16 overflow-hidden rounded-full bg-surface-3">
                    <div
                      class="h-full rounded-full bg-[#88BCFF]"
                      style:width="{Math.max(4, Math.min(100, sharePct))}%"
                    ></div>
                  </div>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>

        {#if groupedRows.length > 0}
          <tfoot>
            <tr class="border-t border-border bg-[#1F1F1F] text-[12px] font-medium text-fg-1 tabular-nums">
              <td class="py-2 pl-3.5 pr-3" colspan="2">
                Filtered total ({groupedRows.length} {groupedRows.length === 1 ? 'group' : 'groups'})
              </td>
              <td class="px-3 py-2 text-right">{totals.calls.toLocaleString()}</td>
              <td class="px-3 py-2 text-right">{formatCompactTokens(totals.inputTokens)}</td>
              <td class="px-3 py-2 text-right">{formatCompactTokens(totals.outputTokens)}</td>
              <td class="px-3 py-2 text-right">
                {formatCompactTokens(totals.cachedTokens)}
                <span class="ml-1 text-[11px] text-[#90CA90]">{totals.cacheHitPercent.toFixed(0)}%</span>
              </td>
              <td class="px-3 py-2 text-right">{formatLatency(totals.avgLatencyMs)}</td>
              <td class="py-2 pl-3 pr-3.5 text-right">{formatCompactTokens(totals.totalTokens)}</td>
            </tr>
          </tfoot>
        {/if}
      </table>
    </div>

    {#if pageCount > 1}
      <div class="flex items-center justify-between border-t border-border px-3.5 py-2 text-[11px] text-fg-3">
        <span class="tabular-nums">Page {currentPage} of {pageCount}</span>
        <div class="flex items-center gap-1.5">
          <button
            type="button"
            class="btn h-7 px-2.5 text-[11px]"
            disabled={!table.getCanPreviousPage()}
            onclick={() => table.previousPage()}
          >
            Prev
          </button>
          <button
            type="button"
            class="btn h-7 px-2.5 text-[11px]"
            disabled={!table.getCanNextPage()}
            onclick={() => table.nextPage()}
          >
            Next
          </button>
        </div>
      </div>
    {/if}
  </div>

  <div class="flex flex-col gap-2.5 rounded-xl border border-border bg-surface-2 p-3.5">
    <div class="flex items-center justify-between">
      <h2 class="text-[13px] font-semibold text-fg-1">What uses the most tokens</h2>
      <span class="text-[11px] text-fg-3 tabular-nums">{data.sinks.length} sinks</span>
    </div>
    {#if data.sinks.length === 0}
      <p class="text-[12px] text-fg-3">No token sinks recorded for {formatMonthLabel(data.month)}.</p>
    {:else}
      <div class="flex flex-col gap-2">
        {#each data.sinks as sink (`${sink.category}:${sink.name}`)}
          <div class="flex items-center gap-3 text-[12px]">
            <div class="flex min-w-0 flex-1 items-center gap-2">
              <span class="truncate font-medium text-fg-1">{sink.name}</span>
              <span class="shrink-0 rounded-full border border-border bg-surface-3 px-1.5 py-0.5 text-[10px] text-fg-3">
                {sink.category}
              </span>
            </div>
            <span class="shrink-0 text-[11px] text-fg-3 tabular-nums">
              {sink.calls.toLocaleString()} {sink.calls === 1 ? 'call' : 'calls'}
            </span>
            <div class="hidden h-1.5 w-28 overflow-hidden rounded-full bg-surface-3 sm:block">
              <div
                class="h-full rounded-full bg-[#E2B623]"
                style:width="{Math.max(2, Math.min(100, sink.sharePercent))}%"
              ></div>
            </div>
            <span class="w-16 shrink-0 text-right font-medium text-fg-1 tabular-nums">
              {formatCompactTokens(sink.tokens)}
            </span>
            <span class="w-10 shrink-0 text-right text-[11px] text-fg-3 tabular-nums">
              {sink.sharePercent.toFixed(0)}%
            </span>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>
