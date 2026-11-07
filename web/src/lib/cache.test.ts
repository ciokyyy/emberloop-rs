import { describe, expect, it } from 'vitest';
import type { UsageRecord } from './api/types';
import { coldStart, latestHit, sessionHit, trendPoints } from './cache';

const row = (seq: number, cached: number, creation: number, fresh: number, hitPercent: number | null): UsageRecord => ({
  seq,
  at: seq * 1000,
  inputTokens: cached + creation + fresh,
  freshInputTokens: fresh,
  cachedTokens: cached,
  cacheCreationTokens: creation,
  outputTokens: 10,
  hitPercent
});

// Oldest first, as the endpoint returns them: a cold first request, then warm ones.
const warm = [
  row(1, 0, 1000, 200, 0),
  row(2, 1000, 100, 100, 1000 / 1200 * 100),
  row(3, 1100, 50, 50, 1100 / 1200 * 100)
];

describe('latestHit', () => {
  it('is the hit percent of the newest request', () => {
    expect(latestHit(warm)).toBeCloseTo(1100 / 1200 * 100);
  });

  it('is null with no rows or when the newest prompt is empty', () => {
    expect(latestHit([])).toBeNull();
    expect(latestHit([row(1, 0, 0, 0, null)])).toBeNull();
  });
});

describe('sessionHit', () => {
  it('weighs every request by its prompt tokens rather than averaging percentages', () => {
    // 2100 cached out of 3600 prompt tokens; an average of the three percentages would differ.
    expect(sessionHit(warm)).toBeCloseTo((2100 / 3600) * 100);
  });

  it('is null when no prompt was counted', () => {
    expect(sessionHit([])).toBeNull();
    expect(sessionHit([row(1, 0, 0, 0, null)])).toBeNull();
  });
});

describe('trendPoints', () => {
  it('keeps one point per request in order, skipping requests without a percent', () => {
    expect(trendPoints([...warm, row(4, 0, 0, 0, null)]).map((point) => point.seq)).toEqual([1, 2, 3]);
    expect(trendPoints(warm)[0]).toEqual({ seq: 1, at: 1000, percent: 0 });
  });
});

describe('coldStart', () => {
  it('flags a first request that wrote the cache without reading it', () => {
    expect(coldStart(warm)).toBe(true);
  });

  it('does not flag a first request that already read the cache, or a provider that wrote nothing', () => {
    expect(coldStart([row(1, 500, 0, 500, 50)])).toBe(false);
    expect(coldStart([row(1, 0, 0, 900, null)])).toBe(false);
    expect(coldStart([])).toBe(false);
  });
});
