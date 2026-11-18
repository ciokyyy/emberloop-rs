import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, expect, it, vi } from 'vitest';
import type { BinariesStatus, SystemStatus, TokenStatus } from '$lib/api/types';

const systemMock = vi.hoisted(() => vi.fn<(refresh?: boolean) => Promise<SystemStatus>>());
const binariesMock = vi.hoisted(() => vi.fn<(refresh?: boolean) => Promise<BinariesStatus>>());
const tokenMock = vi.hoisted(() => vi.fn<(month?: string) => Promise<TokenStatus>>());

vi.mock('$lib/api', () => ({
  api: {
    systemStatus: systemMock,
    binariesStatus: binariesMock,
    tokenStatus: tokenMock
  }
}));

import Status from './Status.svelte';

const SYSTEM_SAMPLE: SystemStatus = {
  sampledAt: 1_760_000_000_000,
  host: {
    hostname: 'devbox',
    os: 'linux/x86_64',
    version: '0.0.99',
    uptimeSeconds: 7200
  },
  cpu: { percent: 34.5, cores: 8, loadAvg1: 1.25, loadAvg5: 1.1, loadAvg15: 0.9 },
  memory: {
    usedBytes: 8 * 1024 ** 3,
    totalBytes: 16 * 1024 ** 3,
    percent: 50,
    cachedBytes: 2 * 1024 ** 3,
    availableBytes: 8 * 1024 ** 3,
    swapUsedBytes: 0,
    swapTotalBytes: 2 * 1024 ** 3
  },
  disk: {
    path: '/data',
    usedBytes: 60 * 1024 ** 3,
    totalBytes: 100 * 1024 ** 3,
    availableBytes: 40 * 1024 ** 3,
    percent: 60
  },
  runtime: { activeSessions: 3, workingSessions: 1, archivedSessions: 12 },
  processes: [
    { pid: 101, name: 'emberloopd', detail: 'daemon · 1 active', cpuPercent: 4.2, rssBytes: 128 * 1024 ** 2, state: 'live' },
    { pid: 202, name: 'agy-proxy', detail: 'session s1', cpuPercent: 12.1, rssBytes: 256 * 1024 ** 2, state: 'working' }
  ],
  storage: {
    rootPath: '/root/.local/share/emberloop-rs',
    totalUsedBytes: 512 * 1024 ** 2,
    diskTotalBytes: 100 * 1024 ** 3,
    diskPercent: 0.5,
    segments: [
      { id: 'sessions', label: 'Sessions', bytes: 300 * 1024 ** 2, percent: 58.6, tone: 'blue' },
      { id: 'archives', label: 'Archives', bytes: 212 * 1024 ** 2, percent: 41.4, tone: 'amber' }
    ]
  }
};

const TOKENS_NOV: TokenStatus = {
  month: '2026-11',
  prevMonth: '2026-10',
  nextMonth: '',
  availableMonths: ['2026-11', '2026-10'],
  summary: {
    totalTokens: 1_500_000,
    inputTokens: 200_000,
    outputTokens: 100_000,
    cachedTokens: 1_150_000,
    cacheCreationTokens: 50_000,
    cacheHitPercent: 82.1,
    totalCalls: 140,
    avgLatencyMs: 2400,
    p95LatencyMs: 6200,
    topSinkName: 'bash',
    topSinkTokens: 640_000,
    topSinkSharePercent: 42.7
  },
  rows: [
    {
      id: 'r1',
      model: 'claude-sonnet-4-5',
      provider: 'anthropic',
      harness: 'claude',
      harnessLabel: 'Claude',
      projectId: 'p1',
      projectName: 'emberloop-rs',
      calls: 100,
      inputTokens: 120_000,
      outputTokens: 60_000,
      cachedTokens: 900_000,
      cacheCreationTokens: 20_000,
      totalTokens: 1_100_000,
      cacheHitPercent: 86.5,
      totalLatencyMs: 250_000,
      avgLatencyMs: 2500,
      p95LatencyMs: 6200
    },
    {
      id: 'r2',
      model: 'gemini-3.8-flash',
      provider: 'google',
      harness: 'agy',
      harnessLabel: 'Antigravity',
      projectId: 'p2',
      projectName: 'sandbox',
      calls: 40,
      inputTokens: 80_000,
      outputTokens: 40_000,
      cachedTokens: 250_000,
      cacheCreationTokens: 30_000,
      totalTokens: 400_000,
      cacheHitPercent: 69.4,
      totalLatencyMs: 86_000,
      avgLatencyMs: 2150,
      p95LatencyMs: 4100
    }
  ],
  sinks: [
    { name: 'bash', category: 'tool', tokens: 640_000, sharePercent: 42.7, calls: 55 },
    { name: 'read', category: 'tool', tokens: 320_000, sharePercent: 21.3, calls: 42 }
  ]
};

const BINARIES_SAMPLE: BinariesStatus = {
  checkedAtMs: 1_760_000_100_000,
  binaries: [
    {
      id: 'figcraft',
      name: 'FigCraft',
      path: '/usr/local/bin/figcraft-cli',
      installedVersion: '0.18.1',
      latestVersion: '0.18.1',
      state: 'upToDate'
    },
    {
      id: 'rtk',
      name: 'rtk',
      path: '/usr/local/bin/rtk',
      installedVersion: '1.1.0',
      latestVersion: '1.2.0',
      state: 'outdated'
    },
    {
      id: 'pi',
      name: 'pi',
      path: '/usr/local/bin/pi',
      installedVersion: '',
      latestVersion: '0.12.16',
      state: 'missing'
    },
    {
      id: 'claude',
      name: 'Claude Code',
      path: '/usr/local/bin/claude',
      installedVersion: '2.1.300',
      latestVersion: '',
      state: 'error',
      error: 'registry.npmjs.org timed out'
    }
  ]
};

beforeEach(() => {
  systemMock.mockReset();
  binariesMock.mockReset();
  tokenMock.mockReset();
  systemMock.mockResolvedValue(SYSTEM_SAMPLE);
  binariesMock.mockResolvedValue(BINARIES_SAMPLE);
  tokenMock.mockResolvedValue(TOKENS_NOV);
});

it('renders System metrics, processes, storage and toggles live polling', async () => {
  render(Status);
  expect(await screen.findByText(/devbox · linux\/x86_64 · v0\.0\.99/)).toBeTruthy();
  expect(screen.getByText('emberloopd')).toBeTruthy();
  expect(screen.getByText('agy-proxy')).toBeTruthy();
  expect(screen.getByText('Sessions')).toBeTruthy();
  expect(screen.getByText('35%')).toBeTruthy();

  const liveBtn = screen.getByRole('button', { name: /Live · 5s/ });
  await fireEvent.click(liveBtn);
  expect(screen.getByRole('button', { name: /Paused/ })).toBeTruthy();
});

it('loads external binaries once, keeps them out of live ticks and refreshes on demand', async () => {
  vi.useFakeTimers();
  try {
    render(Status);
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(binariesMock).toHaveBeenCalledTimes(1));
    expect(binariesMock).toHaveBeenCalledWith(false);
    await vi.waitFor(() => expect(screen.getByText('Up to date')).toBeTruthy());
    expect(screen.getByText('External binaries')).toBeTruthy();
    expect(screen.getByText('Missing')).toBeTruthy();
    expect(screen.getByText('Error')).toBeTruthy();
    expect(screen.getByText('registry.npmjs.org timed out')).toBeTruthy();

    const polled = systemMock.mock.calls.length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(systemMock.mock.calls.length).toBeGreaterThan(polled);
    expect(systemMock).toHaveBeenCalledWith(true);
    expect(binariesMock).toHaveBeenCalledTimes(1);

    await fireEvent.click(screen.getByRole('button', { name: 'Check updates' }));
    await vi.waitFor(() => expect(binariesMock).toHaveBeenCalledWith(true));
  } finally {
    vi.useRealTimers();
  }
});

it('renders external binaries even when system telemetry fails', async () => {
  systemMock.mockRejectedValue(new Error('host telemetry down'));
  render(Status);
  expect(await screen.findByText('host telemetry down')).toBeTruthy();
  expect(await screen.findByText('External binaries')).toBeTruthy();
  expect(await screen.findByText('Up to date')).toBeTruthy();
});

it('reports a failed binaries check without hiding the system view', async () => {
  binariesMock.mockRejectedValue(new Error('binaries offline'));
  render(Status);
  expect(await screen.findByText('binaries offline')).toBeTruthy();
  expect(await screen.findByText(/devbox · linux\/x86_64 · v0\.0\.99/)).toBeTruthy();
});

it('switches to Tokens tab, filters with chips, groups, steps month, and omits dollar signs', async () => {
  const createUrl = vi.fn(() => 'blob:test');
  const revokeUrl = vi.fn();
  Object.defineProperty(URL, 'createObjectURL', { value: createUrl, writable: true });
  Object.defineProperty(URL, 'revokeObjectURL', { value: revokeUrl, writable: true });
  const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});

  render(Status);
  await screen.findByText('emberloopd');

  await fireEvent.click(screen.getByRole('tab', { name: 'Tokens' }));
  expect(await screen.findByText('What uses the most tokens')).toBeTruthy();
  expect(screen.getAllByText('claude-sonnet-4-5').length).toBeGreaterThanOrEqual(2);
  expect(screen.getAllByText('gemini-3.8-flash').length).toBeGreaterThanOrEqual(2);
  expect(document.body.textContent).not.toContain('$');

  const table = screen.getByRole('table');
  const search = screen.getByLabelText('Filter telemetry');
  await fireEvent.input(search, { target: { value: 'gemini' } });
  expect(table.textContent).not.toContain('claude-sonnet-4-5');
  expect(table.textContent).toContain('gemini-3.8-flash');
  expect(screen.getByRole('button', { name: /search: gemini/ })).toBeTruthy();

  await fireEvent.click(screen.getByRole('button', { name: 'Reset' }));
  expect(table.textContent).toContain('claude-sonnet-4-5');

  await fireEvent.change(screen.getByLabelText('Group by'), { target: { value: 'harness' } });
  expect(screen.getAllByText('1 model').length).toBe(2);
  await fireEvent.click(screen.getByRole('button', { name: 'Export CSV' }));
  expect(createUrl).toHaveBeenCalledTimes(1);
  expect(clickSpy).toHaveBeenCalledTimes(1);

  await fireEvent.click(screen.getByRole('button', { name: 'Previous month' }));
  await waitFor(() => expect(tokenMock).toHaveBeenCalledWith('2026-10'));
  clickSpy.mockRestore();
});
