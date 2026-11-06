import { describe, expect, it } from 'vitest';
import { METER_AMBER, METER_QUIET, METER_RED, contextPct, formatTokens, meterTone } from './contextMeter';

describe('contextMeter', () => {
  it('computes a capped percent', () => {
    expect(contextPct(null)).toBe(0);
    expect(contextPct({ used: 5, size: 0 })).toBe(0);
    expect(contextPct({ used: 412_000, size: 1_000_000 })).toBe(41);
    expect(contextPct({ used: 2_000_000, size: 1_000_000 })).toBe(100);
  });
  it('warms at 70 and reddens at 90', () => {
    expect(meterTone(69)).toBe(METER_QUIET);
    expect(meterTone(70)).toBe(METER_AMBER);
    expect(meterTone(90)).toBe(METER_RED);
  });
  it('formats token counts', () => {
    expect(formatTokens(412_000)).toBe('412k');
    expect(formatTokens(1_000_000)).toBe('1M');
    expect(formatTokens(950)).toBe('950');
  });
});
