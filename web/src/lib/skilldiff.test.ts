import { describe, it, expect } from 'vitest';
import { lineDiff } from './skilldiff';

describe('lineDiff', () => {
  it('marks removed and added lines and keeps the common ones', () => {
    expect(lineDiff('a\nb\nc', 'a\nc\nd')).toEqual([
      { kind: 'same', text: 'a' },
      { kind: 'del', text: 'b' },
      { kind: 'same', text: 'c' },
      { kind: 'add', text: 'd' }
    ]);
  });
});
