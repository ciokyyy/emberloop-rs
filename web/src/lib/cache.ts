import type { UsageRecord } from './api/types';

/** One request on the trend: its prompt-cache hit percent (0-100). */
export interface CachePoint {
  seq: number;
  at: number;
  percent: number;
}

/** The prompt tokens of one request: cache reads, cache writes and the fresh remainder. */
const promptOf = (row: UsageRecord): number => row.cachedTokens + row.cacheCreationTokens + row.freshInputTokens;

/** The newest request's hit percent; null with no rows or when its prompt is empty. */
export function latestHit(rows: UsageRecord[]): number | null {
  return rows.at(-1)?.hitPercent ?? null;
}

/** The Session's hit percent over `rows`, weighted by prompt tokens: a long request counts for more
 *  than a short one, so the figure is not the mean of the per-request percentages. */
export function sessionHit(rows: UsageRecord[]): number | null {
  const prompt = rows.reduce((sum, row) => sum + promptOf(row), 0);
  if (prompt === 0) return null;
  return (rows.reduce((sum, row) => sum + row.cachedTokens, 0) / prompt) * 100;
}

/** The per-request trend, oldest first. A request without a percent (an empty prompt, or a provider
 *  that reports no cache fields) has no point, so the line breaks there instead of dropping to 0. */
export function trendPoints(rows: UsageRecord[]): CachePoint[] {
  return rows.flatMap((row) =>
    row.hitPercent === null || row.hitPercent === undefined
      ? []
      : [{ seq: row.seq, at: row.at, percent: row.hitPercent }]
  );
}

/** True when the oldest request wrote the cache and read nothing back: the cache starts cold there,
 *  so its 0% says nothing about the provider. */
export function coldStart(rows: UsageRecord[]): boolean {
  const first = rows[0];
  return first !== undefined && first.cachedTokens === 0 && first.cacheCreationTokens > 0;
}
