import type { ContextBreakdown } from './api/types';

export function readBreakdown(value: unknown): ContextBreakdown | null {
  if (!value || typeof value !== 'object') return null;
  const data = value as Record<string, unknown>;
  const keys = ['system', 'toolDefinitions', 'toolCalls', 'toolResults', 'compression', 'conversation', 'images', 'imageCount', 'estimatedTokens'] as const;
  const out = {} as ContextBreakdown;
  for (const key of keys) {
    const number = data[key];
    if (typeof number !== 'number' || !Number.isFinite(number) || number < 0) return null;
    out[key] = number;
  }
  return out;
}
