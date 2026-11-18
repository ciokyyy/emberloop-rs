import { describe, expect, it } from 'vitest';
import { isOneShot, resolveComposerAction, shouldSend } from './composer';

describe('composer', () => {
  it('sends on Enter and adds a line on Shift+Enter', () => {
    expect(shouldSend({ key: 'Enter', shiftKey: false })).toBe(true);
    expect(shouldSend({ key: 'Enter', shiftKey: true })).toBe(false);
    expect(shouldSend({ key: 'a', shiftKey: false })).toBe(false);
  });

  it('routes a working Session to steer and an idle one to prompt', () => {
    expect(resolveComposerAction(true)).toBe('steer');
    expect(resolveComposerAction(false)).toBe('prompt');
  });

  it('hides the composer for a one-shot goldfish only', () => {
    expect(isOneShot('goldfish')).toBe(true);
    expect(isOneShot('scout')).toBe(false);
    expect(isOneShot(null)).toBe(false);
  });
});
