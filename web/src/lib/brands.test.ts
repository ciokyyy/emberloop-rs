import { describe, expect, it } from 'vitest';
import { brandSvg, brandTint, isColoured, PI_TINT } from './brands';

describe('brand map', () => {
  it('returns SVG markup for every provider and harness we list', () => {
    for (const id of ['claude', 'agy', 'pi', 'anthropic', 'openai-codex', 'openrouter', 'deepseek', 'mistral', 'xai']) {
      expect(brandSvg(id)).toMatch(/^<svg[\s>]/);
    }
  });

  it('keeps harness colours, makes providers follow the text colour, and tints pi', () => {
    expect(isColoured('claude')).toBe(true);
    expect(isColoured('agy')).toBe(true);
    expect(isColoured('openrouter')).toBe(false);
    expect(brandSvg('openrouter')).toContain('fill="currentColor"');
    expect(brandTint('pi')).toBe(PI_TINT);
    expect(brandTint('openrouter')).toBeUndefined();
  });

  it('returns nothing for an unknown id so the caller can fall back to a letter', () => {
    expect(brandSvg('no-such-provider')).toBeUndefined();
  });
});
