import type { TokenTelemetryRow } from '$lib/api/types';

export type GroupMode = 'model-harness' | 'model' | 'harness' | 'project';

export interface TokenFilterState {
  search: string;
  harness: string;
  model: string;
  minLatencyMs: number;
  minCalls: number;
}

export interface GroupedTokenRow {
  id: string;
  primary: string;
  secondary: string;
  iconProvider: string;
  iconSource: string;
  harness: string;
  harnessLabel: string;
  projectLabel: string;
  calls: number;
  inputTokens: number;
  outputTokens: number;
  cachedTokens: number;
  cacheCreationTokens: number;
  totalTokens: number;
  cacheHitPercent: number;
  avgLatencyMs: number;
  p95LatencyMs: number;
}

export interface TokenSummaryTotals {
  calls: number;
  inputTokens: number;
  outputTokens: number;
  cachedTokens: number;
  totalTokens: number;
  cacheHitPercent: number;
  avgLatencyMs: number;
  p95LatencyMs: number;
}

export const DEFAULT_TOKEN_FILTERS: TokenFilterState = {
  search: '',
  harness: 'all',
  model: 'all',
  minLatencyMs: 0,
  minCalls: 0
};

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let idx = 0;
  let value = bytes;
  while (value >= 1024 && idx < units.length - 1) {
    value /= 1024;
    idx += 1;
  }
  const decimals = idx === 0 ? 0 : value >= 100 ? 0 : 1;
  return `${value.toFixed(decimals)} ${units[idx]}`;
}

export function formatCompactTokens(tokens: number): string {
  if (!Number.isFinite(tokens) || tokens <= 0) return '0';
  if (tokens >= 1_000_000_000) return `${(tokens / 1_000_000_000).toFixed(2)}B`;
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(2)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}K`;
  return String(Math.round(tokens));
}

export function formatLatency(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return '–';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const secs = ms / 1000;
  if (secs < 60) return `${secs.toFixed(secs >= 10 ? 1 : 2).replace(/\.0$/, '')}s`;
  const mins = Math.floor(secs / 60);
  const rem = Math.round(secs % 60);
  return `${mins}m ${rem}s`;
}

export function formatUptime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 60) return `${Math.max(0, Math.floor(seconds))}s`;
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const mins = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

export function formatMonthLabel(yyyyMm: string): string {
  const match = /^(\d{4})-(\d{2})$/.exec(yyyyMm);
  if (!match) return yyyyMm;
  const date = new Date(Date.UTC(Number(match[1]), Number(match[2]) - 1, 1));
  return date.toLocaleString('en-US', { month: 'long', year: 'numeric', timeZone: 'UTC' });
}

export function ringStrokeColor(percent: number): string {
  if (percent >= 90) return 'var(--status-failed)';
  if (percent >= 70) return 'var(--status-needs)';
  return 'var(--status-done)';
}

export function meterTextTone(percent: number): string {
  if (percent >= 90) return 'text-status-failed';
  if (percent >= 70) return 'text-status-needs';
  return 'text-status-done';
}

export function uniqueHarnesses(rows: TokenTelemetryRow[]): { value: string; label: string }[] {
  const map = new Map<string, string>();
  for (const row of rows) {
    if (!map.has(row.harness)) map.set(row.harness, row.harnessLabel);
  }
  return Array.from(map.entries())
    .map(([value, label]) => ({ value, label }))
    .sort((a, b) => a.label.localeCompare(b.label));
}

export function uniqueModels(rows: TokenTelemetryRow[]): string[] {
  return Array.from(new Set(rows.map((row) => row.model))).sort((a, b) => a.localeCompare(b));
}

export function filterTokenRows(
  rows: TokenTelemetryRow[],
  filters: TokenFilterState
): TokenTelemetryRow[] {
  const q = filters.search.trim().toLowerCase();
  return rows.filter((row) => {
    if (filters.harness !== 'all' && row.harness !== filters.harness) return false;
    if (filters.model !== 'all' && row.model !== filters.model) return false;
    if (filters.minLatencyMs > 0 && row.avgLatencyMs < filters.minLatencyMs) return false;
    if (filters.minCalls > 0 && row.calls < filters.minCalls) return false;
    if (q) {
      const hay = `${row.model} ${row.provider} ${row.harness} ${row.harnessLabel} ${row.projectName}`.toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  });
}

interface BucketAcc {
  id: string;
  rows: TokenTelemetryRow[];
}

export function groupTokenRows(rows: TokenTelemetryRow[], mode: GroupMode): GroupedTokenRow[] {
  const buckets = new Map<string, BucketAcc>();
  for (const row of rows) {
    const key =
      mode === 'model-harness'
        ? `${row.model}::${row.harness}`
        : mode === 'model'
          ? row.model
          : mode === 'harness'
            ? row.harness
            : row.projectId;
    const acc = buckets.get(key) ?? { id: key, rows: [] };
    acc.rows.push(row);
    buckets.set(key, acc);
  }

  return Array.from(buckets.values()).map(({ id, rows: group }) => {
    const first = group[0];
    let calls = 0;
    let inputTokens = 0;
    let outputTokens = 0;
    let cachedTokens = 0;
    let cacheCreationTokens = 0;
    let totalTokens = 0;
    let totalLatencyMs = 0;
    let p95LatencyMs = 0;
    const models = new Set<string>();
    const harnesses = new Set<string>();
    const projects = new Set<string>();

    for (const r of group) {
      calls += r.calls;
      inputTokens += r.inputTokens;
      outputTokens += r.outputTokens;
      cachedTokens += r.cachedTokens;
      cacheCreationTokens += r.cacheCreationTokens;
      totalTokens += r.totalTokens;
      totalLatencyMs += r.totalLatencyMs;
      if (r.p95LatencyMs > p95LatencyMs) p95LatencyMs = r.p95LatencyMs;
      models.add(r.model);
      harnesses.add(r.harnessLabel);
      projects.add(r.projectName);
    }

    const cacheHitPercent =
      inputTokens > 0 ? Math.min(100, (cachedTokens / inputTokens) * 100) : 0;
    const avgLatencyMs = calls > 0 ? totalLatencyMs / calls : 0;
    const projectLabel =
      projects.size === 1 ? first.projectName : `${projects.size} projects`;
    const harnessList = Array.from(harnesses);
    const harnessLabel =
      harnessList.length === 1 ? harnessList[0] : `${harnessList.length} harnesses`;

    const primary =
      mode === 'harness'
        ? first.harnessLabel
        : mode === 'project'
          ? first.projectName
          : first.model;
    const secondary =
      mode === 'model-harness'
        ? projectLabel
        : mode === 'model'
          ? harnessList.join(', ')
          : mode === 'harness'
            ? `${models.size} ${models.size === 1 ? 'model' : 'models'}`
            : `${models.size} ${models.size === 1 ? 'model' : 'models'} · ${harnessLabel}`;

    return {
      id,
      primary,
      secondary,
      iconProvider: mode === 'harness' || mode === 'project' ? first.harness : first.provider,
      iconSource: first.harness,
      harness: first.harness,
      harnessLabel,
      projectLabel,
      calls,
      inputTokens,
      outputTokens,
      cachedTokens,
      cacheCreationTokens,
      totalTokens,
      cacheHitPercent,
      avgLatencyMs,
      p95LatencyMs
    };
  });
}

export function summarizeTokenRows(rows: GroupedTokenRow[]): TokenSummaryTotals {
  let calls = 0;
  let inputTokens = 0;
  let outputTokens = 0;
  let cachedTokens = 0;
  let totalTokens = 0;
  let weightedLatency = 0;
  let p95LatencyMs = 0;

  for (const r of rows) {
    calls += r.calls;
    inputTokens += r.inputTokens;
    outputTokens += r.outputTokens;
    cachedTokens += r.cachedTokens;
    totalTokens += r.totalTokens;
    weightedLatency += r.avgLatencyMs * r.calls;
    if (r.p95LatencyMs > p95LatencyMs) p95LatencyMs = r.p95LatencyMs;
  }

  return {
    calls,
    inputTokens,
    outputTokens,
    cachedTokens,
    totalTokens,
    cacheHitPercent: inputTokens > 0 ? Math.min(100, (cachedTokens / inputTokens) * 100) : 0,
    avgLatencyMs: calls > 0 ? weightedLatency / calls : 0,
    p95LatencyMs
  };
}

function csvCell(value: string | number): string {
  const s = String(value);
  return /[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

export function rowsToCsv(rows: GroupedTokenRow[]): string {
  const header = [
    'Name',
    'Harness',
    'Scope',
    'Calls',
    'Input Tokens',
    'Output Tokens',
    'Cached Tokens',
    'Total Tokens',
    'Cache Hit %',
    'Avg Latency ms',
    'P95 Latency ms'
  ];
  const lines = [header.join(',')];
  for (const r of rows) {
    lines.push(
      [
        r.primary,
        r.harnessLabel,
        r.projectLabel,
        r.calls,
        r.inputTokens,
        r.outputTokens,
        r.cachedTokens,
        r.totalTokens,
        r.cacheHitPercent.toFixed(1),
        Math.round(r.avgLatencyMs),
        Math.round(r.p95LatencyMs)
      ]
        .map(csvCell)
        .join(',')
    );
  }
  return lines.join('\n');
}
