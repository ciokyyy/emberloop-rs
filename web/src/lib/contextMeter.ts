// The context meter's voice: the ring warns as the window fills.
export const METER_AMBER = 'text-status-needs';
export const METER_RED = 'text-status-failed';
export const METER_QUIET = 'text-fg-2';

export interface Usage {
  used: number;
  size: number;
}

/** Rounded percent of the window used, capped at 100. */
export function contextPct(usage: Usage | null | undefined): number {
  if (!usage || !(usage.size > 0)) return 0;
  return Math.min(100, Math.round((usage.used / usage.size) * 100));
}

/** Colour class of the ring: warm from 70%, red from 90%. */
export function meterTone(pct: number): string {
  return pct >= 90 ? METER_RED : pct >= 70 ? METER_AMBER : METER_QUIET;
}

/** `412k / 1M` style label. */
export function formatTokens(count: number): string {
  if (count >= 1_000_000) return `${+(count / 1_000_000).toFixed(1)}M`;
  if (count >= 1000) return `${Math.round(count / 1000)}k`;
  return String(count);
}

/** A model change takes effect before the next usage response arrives. */
export function chosenWindow(modelWindow = 0, responseWindow = 0): number {
  return modelWindow || responseWindow || 0;
}
