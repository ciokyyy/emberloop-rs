import { describe, expect, it } from 'vitest';
import type { FeedItem } from './feed';
import { runOf, segments } from './toolRuns';

const tool = (id: string, extra: Partial<FeedItem> = {}): FeedItem => ({
  id,
  kind: 'tool',
  tool: 'read',
  name: 'mcp__el__read',
  input: '{"path":"a.rs"}',
  output: 'x',
  status: 'completed',
  finished: true,
  ...extra
});

describe('segments', () => {
  it('folds consecutive tools and thinking into one block between messages', () => {
    const items: FeedItem[] = [
      { id: 'u', kind: 'user', text: 'go' },
      { id: 'th', kind: 'thinking', text: 'hmm' },
      tool('t1'),
      tool('t2', { status: 'running', finished: false }),
      { id: 'a', kind: 'assistant', text: 'done' },
      tool('t3')
    ];
    const out = segments(items);
    expect(out.map((s) => s.type)).toEqual(['item', 'tools', 'item', 'tools']);
    const block = out[1];
    expect(block.type === 'tools' && block.runs.map((r) => r.name)).toEqual(['thinking', 'read', 'read']);
    expect(block.type === 'tools' && block.running).toBe(true);
  });

  it('maps a tool item to a run and marks thinking', () => {
    expect(runOf(tool('t', { elapsedMs: 1200 }))).toMatchObject({ name: 'read', elapsedMs: 1200, status: 'completed' });
    expect(runOf({ id: 'x', kind: 'thinking', text: 'why' })).toMatchObject({ thinking: true, output: 'why' });
  });
});
