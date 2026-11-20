import { describe, expect, it } from 'vitest';
import { expandSlash, isOneShot, resolveComposerAction, shouldSend, slashQuery } from './composer';

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

  it('reads the word after a leading slash only while the cursor sits in it', () => {
    expect(slashQuery('/flaky', 6)).toBe('flaky');
    expect(slashQuery('/flaky', 3)).toBe('flaky');
    expect(slashQuery('/', 1)).toBe('');
    expect(slashQuery('/flaky now go', 13)).toBeNull();
    expect(slashQuery('ask /flaky', 10)).toBeNull();
  });

  it('expands a known slash name and leaves unknown or mid-text slashes alone', () => {
    expect(expandSlash('/ember-flow build it', ['ember-flow'])).toBe(
      'Read `el://skills/ember-flow` and follow it.\n\nbuild it'
    );
    expect(expandSlash('/ember-flow', ['ember-flow'])).toBe(
      'Read `el://skills/ember-flow` and follow it.'
    );
    expect(expandSlash('/nope build it', ['ember-flow'])).toBe('/nope build it');
    expect(expandSlash('please /ember-flow', ['ember-flow'])).toBe('please /ember-flow');
  });
});
