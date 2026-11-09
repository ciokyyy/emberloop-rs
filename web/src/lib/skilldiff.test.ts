import { describe, it, expect } from 'vitest';
import { lineDiff, skillDiffText, skillManageView } from './skilldiff';

describe('skillDiffText', () => {
  it('shows each op header and the old and new text of a patch', () => {
    const ops = [
      { op: 'patch', name: 'deploy', old_string: 'a\nb', new_string: 'c' },
      { op: 'create', name: 'new-one', content: 'x' }
    ];
    expect(skillDiffText(ops)).toBe('deploy · patch\n- a\n- b\n+ c\nnew-one · create');
  });

  it('returns nothing for missing or non-list ops', () => {
    expect(skillDiffText(undefined)).toBe('');
    expect(skillDiffText('x')).toBe('');
  });
});
describe('skillManageView', () => {
  it('reads the touched skill names and the diff from the call arguments', () => {
    const input = JSON.stringify({ ops: [{ op: 'patch', name: 'deploy', old_string: 'a', new_string: 'b' }] });
    expect(skillManageView(input)).toEqual({ path: 'deploy', diff: 'deploy · patch\n- a\n+ b' });
  });

  it('returns empty view for arguments that do not parse', () => {
    expect(skillManageView('not json')).toEqual({ path: '', diff: '' });
  });
});

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
