import { describe, expect, it } from 'vitest';
import { insertAt, pickAudioType } from './voice';

describe('pickAudioType', () => {
  it('takes the first type the browser supports', () => {
    expect(pickAudioType((type) => type === 'audio/webm')).toBe('audio/webm');
    expect(pickAudioType(() => true)).toBe('audio/webm;codecs=opus');
  });

  it('returns undefined when nothing is supported', () => {
    expect(pickAudioType(() => false)).toBeUndefined();
  });
});

describe('insertAt', () => {
  it('inserts at the cursor and keeps the rest', () => {
    expect(insertAt('hello world', 6, 6, 'big ')).toBe('hello big world');
  });

  it('replaces the selection', () => {
    expect(insertAt('one two three', 4, 7, 'x')).toBe('one x three');
  });

  it('adds a space when the text before has none', () => {
    expect(insertAt('', 0, 0, 'spoken')).toBe('spoken');
    expect(insertAt('ab', 2, 2, 'cd')).toBe('ab cd');
    expect(insertAt('ab ', 3, 3, 'cd')).toBe('ab cd');
  });
});
