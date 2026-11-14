import { describe, expect, it } from 'vitest';
import type { TokenTelemetryRow } from '$lib/api/types';
import {
  DEFAULT_TOKEN_FILTERS,
  filterTokenRows,
  formatBytes,
  formatCompactTokens,
  formatLatency,
  formatMonthLabel,
  formatUptime,
  groupTokenRows,
  ringStrokeColor,
  rowsToCsv,
  summarizeTokenRows
} from './status';

const sampleRows: TokenTelemetryRow[] = [
  {
    id: 'r1',
    model: 'claude-sonnet-4-5',
    provider: 'anthropic',
    harness: 'agy',
    harnessLabel: 'Antigravity',
    projectId: 'p1',
    projectName: 'emberloop-rs',
    calls: 120,
    inputTokens: 500_000,
    outputTokens: 50_000,
    cachedTokens: 400_000,
    cacheCreationTokens: 0,
    totalTokens: 550_000,
    cacheHitPercent: 80,
    totalLatencyMs: 240_000,
    avgLatencyMs: 2000,
    p95LatencyMs: 4500
  },
  {
    id: 'r2',
    model: 'claude-sonnet-4-5',
    provider: 'anthropic',
    harness: 'agy',
    harnessLabel: 'Antigravity',
    projectId: 'p2',
    projectName: 'website',
    calls: 20,
    inputTokens: 50_000,
    outputTokens: 10_000,
    cachedTokens: 30_000,
    cacheCreationTokens: 0,
    totalTokens: 60_000,
    cacheHitPercent: 60,
    totalLatencyMs: 80_000,
    avgLatencyMs: 4000,
    p95LatencyMs: 7000
  },
  {
    id: 'r3',
    model: 'gpt-5-codex',
    provider: 'openai-codex',
    harness: 'pi',
    harnessLabel: 'pi-rs',
    projectId: 'p1',
    projectName: 'emberloop-rs',
    calls: 5,
    inputTokens: 10_000,
    outputTokens: 2_000,
    cachedTokens: 0,
    cacheCreationTokens: 0,
    totalTokens: 12_000,
    cacheHitPercent: 0,
    totalLatencyMs: 4_000,
    avgLatencyMs: 800,
    p95LatencyMs: 1200
  }
];

describe('status formatting helpers', () => {
  it('formats bytes, tokens, latency, uptime, month, and ring colors', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatCompactTokens(420)).toBe('420');
    expect(formatCompactTokens(14_250)).toBe('14.3K');
    expect(formatCompactTokens(2_840_000)).toBe('2.84M');
    expect(formatLatency(0)).toBe('–');
    expect(formatLatency(850)).toBe('850ms');
    expect(formatLatency(2450)).toBe('2.45s');
    expect(formatUptime(90000)).toBe('1d 1h');
    expect(formatMonthLabel('2026-11')).toBe('November 2026');
    expect(ringStrokeColor(42)).toBe('var(--status-done)');
    expect(ringStrokeColor(75)).toBe('var(--status-needs)');
    expect(ringStrokeColor(92)).toBe('var(--status-failed)');
  });
});

describe('token filtering, grouping, summary, and CSV export', () => {
  it('filters by search query, harness, model, latency, and call threshold', () => {
    expect(filterTokenRows(sampleRows, DEFAULT_TOKEN_FILTERS)).toHaveLength(3);
    expect(
      filterTokenRows(sampleRows, { ...DEFAULT_TOKEN_FILTERS, search: 'website' }).map((r) => r.id)
    ).toEqual(['r2']);
    expect(
      filterTokenRows(sampleRows, { ...DEFAULT_TOKEN_FILTERS, harness: 'pi' }).map((r) => r.id)
    ).toEqual(['r3']);
    expect(
      filterTokenRows(sampleRows, { ...DEFAULT_TOKEN_FILTERS, minLatencyMs: 3000 }).map((r) => r.id)
    ).toEqual(['r2']);
    expect(
      filterTokenRows(sampleRows, { ...DEFAULT_TOKEN_FILTERS, minCalls: 10 }).map((r) => r.id)
    ).toEqual(['r1', 'r2']);
  });

  it('groups rows by model × harness and recomputes weighted latency & cache hit', () => {
    const grouped = groupTokenRows(sampleRows, 'model-harness');
    expect(grouped).toHaveLength(2);
    const sonnet = grouped.find((r) => r.primary === 'claude-sonnet-4-5');
    expect(sonnet?.calls).toBe(140);
    expect(sonnet?.totalTokens).toBe(610_000);
    expect(sonnet?.projectLabel).toBe('2 projects');
    expect(sonnet?.p95LatencyMs).toBe(7000);
    expect(Math.round(sonnet?.avgLatencyMs ?? 0)).toBe(Math.round(320_000 / 140));
    expect(sonnet?.cacheHitPercent).toBeCloseTo((430_000 / 550_000) * 100, 5);

    const byProject = groupTokenRows(sampleRows, 'project');
    expect(byProject).toHaveLength(2);
    const totals = summarizeTokenRows(grouped);
    expect(totals.calls).toBe(145);
    expect(totals.totalTokens).toBe(622_000);
    expect(totals.cacheHitPercent).toBeCloseTo((430_000 / 560_000) * 100, 5);
  });
  it('exports CSV rows safely without dollar pricing columns', () => {
    const grouped = groupTokenRows(sampleRows, 'model-harness');
    const csv = rowsToCsv(grouped);
    expect(csv).not.toMatch(/cost|price|\$/i);
    expect(csv.split('\n')).toHaveLength(3);
    expect(csv).toContain('claude-sonnet-4-5,Antigravity,2 projects,140');
  });
});
