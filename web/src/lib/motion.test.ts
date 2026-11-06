import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  collapse, fadeAt, flip, flipAnimation, flipReady, motionParams, rise, rowEnter, slide, slideDown, slideUp
} from './motion';

const root = document.documentElement.style;
const setTokens = () => {
  root.setProperty('--dur-instant', '100ms');
  root.setProperty('--dur-standard', '180ms');
  root.setProperty('--dur-emphasis', '320ms');
  root.setProperty('--ease-smooth', 'cubic-bezier(0.22, 1, 0.36, 1)');
};
const reducedMotion = () =>
  vi.stubGlobal('matchMedia', (query: string) => ({ matches: query.includes('prefers-reduced-motion') }));
const node = () => document.body.appendChild(document.createElement('div'));
const css = (t: { css?: (t: number, u: number) => string }, at: number) => t.css!(at, 1 - at);

afterEach(() => {
  for (const name of ['--dur-instant', '--dur-standard', '--dur-emphasis', '--ease-smooth']) root.removeProperty(name);
  vi.unstubAllGlobals();
});

describe('motionParams', () => {
  it('maps every step to its token and the easing to the curve', () => {
    setTokens();
    expect(motionParams('instant').duration).toBe(100);
    expect(motionParams('standard').duration).toBe(180);
    expect(motionParams('emphasis').duration).toBe(320);
    const { easing } = motionParams();
    expect([easing(0), easing(1)]).toEqual([0, 1]);
    expect(easing(0.5)).toBeGreaterThan(0.5);
  });

  it('reads a token the minifier rewrote in seconds', () => {
    root.setProperty('--dur-standard', '.18s');
    root.setProperty('--dur-emphasis', '.32s');
    expect(motionParams('standard').duration).toBe(180);
    expect(motionParams('emphasis').duration).toBe(320);
  });

  it('is zero under reduced motion, also when turned on after the first read', () => {
    setTokens();
    expect(motionParams('standard').duration).toBe(180);
    reducedMotion();
    expect(motionParams('standard').duration).toBe(0);
    expect(slide(null, rowEnter).duration).toBe(0);
  });

  it('is zero when the token is missing', () => {
    expect(motionParams('emphasis').duration).toBe(0);
  });
});

describe('transitions', () => {
  it('slide travels on the emphasis step, on either axis', () => {
    setTokens();
    const t = slide(null);
    expect(t.duration).toBe(320);
    expect(css(t, 0)).toContain('translateY(8px)');
    expect(css(t, 1)).toContain('translateY(0px)');
    expect(css(slide(null, { axis: 'x', amount: 12 }), 0)).toContain('translateX(12px)');
    expect(slide(null, rowEnter).duration).toBe(180);
  });

  it('rise, slideUp and slideDown differ in travel; skip ends them at once', () => {
    setTokens();
    expect(css(rise(null), 0)).toContain('translateY(8px)');
    expect(css(slideUp(null), 0)).toContain('translateY(10px)');
    expect(css(slideDown(null), 0)).toContain('translateY(-8px)');
    expect(rise(null, { skip: true }).duration).toBe(0);
    expect(slideUp(null, { skip: true }).duration).toBe(0);
  });

  it('fadeAt and collapse run on the standard step and stop under reduced motion', () => {
    setTokens();
    expect(fadeAt(node()).duration).toBe(180);
    expect(css(fadeAt(node()), 0)).toBe('opacity: 0');
    expect(collapse(node()).duration).toBe(180);
    reducedMotion();
    for (const t of [rise(null), slideUp(null), slideDown(null), fadeAt(node()), collapse(node())]) {
      expect(t.duration).toBe(0);
    }
  });
});

describe('flip', () => {
  it('is the binding animate:flip calls, with the scale as parameters', () => {
    expect(flip).toBeTypeOf('function');
    setTokens();
    expect(flipAnimation().duration).toBe(180);
    reducedMotion();
    expect(flipAnimation().duration).toBe(0);
  });

  it('flipReady gives a row the animation list Svelte asks for', () => {
    const el = document.createElement('div');
    flipReady(el);
    expect(el.getAnimations()).toEqual([]);
  });
});
