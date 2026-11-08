import { describe, expect, it } from 'vitest';
import { applyEvent, applySnapshot, emptyFeedState, type FeedState } from './feed';
import type { SessionEvent } from './api/types';

const ev = (kind: SessionEvent['kind'], data: Record<string, unknown>, seq = 1): SessionEvent => ({
  v: 1,
  seq,
  sessionId: 's1',
  kind,
  at: 0,
  data
});

const run = (...events: SessionEvent[]): FeedState =>
  events.reduce((state, event) => applyEvent(state, event), emptyFeedState());

describe('feed reducer', () => {
  it('appends assistant chunks into one live row', () => {
    const state = run(ev('assistant_chunk', { text: 'Hel' }), ev('assistant_chunk', { text: 'lo' }, 2));
    expect(state.items).toHaveLength(1);
    expect(state.items[0]).toMatchObject({ kind: 'assistant', text: 'Hello', live: true });
  });

  it('closes the live row and records image count on a prompt', () => {
    const state = run(
      ev('assistant_chunk', { text: 'done' }),
      ev('prompt', { text: 'go on', imagesCount: 2 }, 2)
    );
    expect(state.items[0]).toMatchObject({ kind: 'assistant', live: false });
    expect(state.items[1]).toMatchObject({ kind: 'user', text: 'go on', imagesCount: 2 });
  });

  it('names an agy or pi tool without its server prefix and summarises it from its input', () => {
    const state = run(
      ev('tool_started', { id: 't2', name: 'el_bash', input: { command: 'ls -la' } }),
      ev('tool_finished', { id: 't2', output: 'List directory' }, 2),
      ev('tool_started', { id: 't3', name: 'mcp__el__read', input: { path: 'src/lib.rs' } }, 3),
      ev('tool_finished', { id: 't3', output: '1  fn main()' }, 4)
    );
    expect(state.items[0]).toMatchObject({ kind: 'tool', tool: 'bash', summary: 'ls -la', finished: true });
    expect(state.items[1]).toMatchObject({ kind: 'tool', tool: 'read', summary: 'src/lib.rs', finished: true });
  });
  it('merges tool_finished into the tool_started row', () => {
    const state = run(
      ev('tool_started', { id: 't1', name: 'read', summary: 'src/a.rs', meta: 'lines 1-4' }),
      ev('tool_finished', { id: 't1', name: 'read', output: '1  a', meta: 'lines 1-4 · 3 ms' }, 2)
    );
    expect(state.items).toHaveLength(1);
    expect(state.items[0]).toMatchObject({
      kind: 'tool',
      tool: 'read',
      summary: 'src/a.rs',
      body: '1  a',
      meta: 'lines 1-4 · 3 ms',
      finished: true
    });
  });

  it('creates a tool row when only the finish event arrives', () => {
    const state = run(ev('tool_finished', { name: 'bash', summary: 'cargo test', output: 'ok' }));
    expect(state.items).toHaveLength(1);
    expect(state.items[0]).toMatchObject({ kind: 'tool', tool: 'bash', summary: 'cargo test', finished: true });
  });
  it('reads a Claude tool result given as text blocks', () => {
    const state = run(
      ev('tool_started', { id: 'c1', name: 'mcp__el__bash', input: { command: 'ls' } }),
      ev('tool_finished', { id: 'c1', output: [{ type: 'text', text: 'a.txt' }, { type: 'text', text: 'b.txt' }] }, 2)
    );
    expect(state.items[0]).toMatchObject({ kind: 'tool', tool: 'bash', body: 'a.txt\nb.txt', finished: true });
  });

  it('moves a queued steer to its delivery route', () => {
    const state = run(
      ev('steer_queued', { id: 'q1', text: 'hold on' }),
      ev('steer_delivered', { id: 'q1', route: 'injected' }, 2)
    );
    expect(state.items).toHaveLength(1);
    expect(state.items[0]).toMatchObject({ kind: 'steer', text: 'hold on', route: 'injected' });
  });

  it('carries the prompt-cache counts of the latest request', () => {
    const state = run(
      ev('usage', { inputTokens: 1000, cachedTokens: 600, cacheCreationTokens: 100, contextWindow: 1_000_000 })
    );
    expect(state.usage).toEqual({ used: 1000, window: 1_000_000, cachedTokens: 600, cacheCreationTokens: 100 });
  });

  it('leaves the cache counts out when the provider reports none', () => {
    expect(run(ev('usage', { inputTokens: 1000, contextWindow: 1_000_000 })).usage).toEqual({
      used: 1000,
      window: 1_000_000
    });
  });

  it('tracks status and usage', () => {
    const state = run(
      ev('status', { state: 'working' }),
      ev('usage', { inputTokens: 412_000, contextWindow: 1_000_000 }, 2)
    );
    expect(state.status).toBe('working');
    expect(state.usage).toEqual({ used: 412_000, window: 1_000_000 });
    expect(applyEvent(state, ev('status', { state: 'cancelled' }, 3)).status).toBe('idle');
  });

  it('keeps compression, notice and error rows', () => {
    const state = run(
      ev('compression', { text: 'Compressed 38 turns.' }),
      ev('notice', { id: 'n1', text: 'bg1 exited', route: 'idle' }, 2),
      ev('error', { message: 'daemon lost' }, 3)
    );
    expect(state.items.map((item) => item.kind)).toEqual(['compression', 'notice', 'error']);
    expect(state.items[2].text).toBe('daemon lost');
  });

  it('renders folds and nudges as tool rows', () => {
    const state = run(
      ev('compression', {
        action: 'compress',
        args: { ranges: [{ start: 1, end: 40 }] },
        result: 'Folded 39 turns.'
      }),
      ev('compression', { action: 'nudge', kind: 'first', text: 'nudge: first', tokens: 210_000 }, 2)
    );
    expect(state.items[0]).toMatchObject({
      action: 'compress',
      text: 'turns 1–39',
      body: 'Folded 39 turns.'
    });
    expect(state.items[1]).toMatchObject({ action: 'nudge', text: 'first', meta: '210000 tokens' });
  });

  it('merges repeated tool_started events and reads agy output objects', () => {
    const state = run(
      ev('tool_started', { id: 'c1', name: 'mcp__el__read', input: {} }, 1),
      ev('tool_started', { id: 'c1', name: '', input: { path: 'a.rs' } }, 2),
      ev('tool_finished', { id: 'c1', status: 'completed', output: { text: 'line' } }, 3)
    );
    expect(state.items).toHaveLength(1);
    expect(state.items[0]).toMatchObject({
      name: 'mcp__el__read',
      input: '{"path":"a.rs"}',
      output: 'line',
      status: 'completed'
    });
  });

  it('streams thinking into its own live row and closes it when the turn goes idle', () => {
    const state = run(
      ev('thinking_chunk', { text: 'let me ' }, 1),
      ev('thinking_chunk', { text: 'think' }, 2),
      ev('assistant_chunk', { text: 'ok' }, 3),
      ev('status', { state: 'idle' }, 4)
    );
    expect(state.items.map((item) => item.kind)).toEqual(['thinking', 'assistant']);
    expect(state.items[0]).toMatchObject({ text: 'let me think', live: false });
    expect(state.items[1].live).toBe(false);
  });

  it('marks a failed tool and measures its elapsed time', () => {
    const state = run(
      { ...ev('tool_started', { id: 'b', name: 'bash', input: { command: 'false' } }, 1), at: 1000 },
      { ...ev('tool_finished', { id: 'b', status: 'failed', output: 'exit code: 1' }, 2), at: 3500 }
    );
    expect(state.items[0]).toMatchObject({ status: 'failed', elapsedMs: 2500 });
  });
});

describe('todos', () => {
  const items = [
    { id: 't1', content: 'read the spec', status: 'completed' },
    { id: 't2', content: 'write the parser', status: 'in_progress' }
  ];

  it('keeps the whole list from a todos event, with no feed row', () => {
    const state = run(ev('todos', { items }));
    expect(state.todos).toEqual(items);
    expect(state.items).toEqual([]);
  });

  it('replaces the list on the next todos event', () => {
    const state = run(ev('todos', { items }), ev('todos', { items: [] }, 2));
    expect(state.todos).toEqual([]);
  });

  it('ignores a malformed item and defaults an unknown status', () => {
    const state = run(ev('todos', { items: [{ content: 'no id' }, { id: 't1', content: 'x', status: 'later' }] }));
    expect(state.todos).toEqual([{ id: 't1', content: 'x', status: 'pending' }]);
  });

  it('restores the list from the open snapshot', () => {
    const state = applySnapshot(emptyFeedState(), { todos: { items } });
    expect(state.todos).toEqual(items);
  });
});

describe('todo tool row', () => {
  it('keeps the row to the first line of the full list', () => {
    const state = run(
      ev('tool_started', { id: 't1', name: 'el_todo', input: { action: 'set' } }),
      ev('tool_finished', { id: 't1', output: '2 pending · 0 in progress · 0 completed · 0 cancelled\nt1 [ ] one\nt2 [ ] two' }, 2)
    );
    expect(state.items[0].output).toBe('2 pending · 0 in progress · 0 completed · 0 cancelled');
  });
});

describe('open snapshot', () => {
  it('seeds working status and usage that the tail window did not contain', () => {
    // The window holds only chunks: no status or usage event, so the reducer alone says idle.
    const windowed = run(ev('assistant_chunk', { text: 'x' }));
    expect(windowed.status).toBe('idle');
    const state = applySnapshot(windowed, {
      status: { state: 'working' },
      usage: { inputTokens: 270000, contextWindow: 1_000_000 }
    });
    expect(state.status).toBe('working');
    expect(state.usage).toEqual({ used: 270000, window: 1_000_000 });
    // Items are untouched: a snapshot is not a feed row.
    expect(state.items).toEqual(windowed.items);
  });

  it('keeps what the window said when the snapshot has nothing', () => {
    const windowed = run(ev('status', { state: 'working' }));
    expect(applySnapshot(windowed, { status: null, usage: null })).toEqual(windowed);
  });

  it('carries the prompt-cache counts of the latest request', () => {
    const state = applySnapshot(run(ev('status', { state: 'working' })), {
      usage: { inputTokens: 1000, cachedTokens: 600, cacheCreationTokens: 100, contextWindow: 1_000_000 }
    });
    expect(state.usage).toEqual({ used: 1000, window: 1_000_000, cachedTokens: 600, cacheCreationTokens: 100 });
  });

  it('does not close the live row when the snapshot says working', () => {
    const windowed = run(ev('assistant_chunk', { text: 'x' }));
    const state = applySnapshot(windowed, { status: { state: 'working' }, usage: null });
    expect(state.items[0]).toMatchObject({ live: true });
  });

  it('reads a cancelled status as idle', () => {
    expect(run(ev('status', { state: 'cancelled' })).status).toBe('idle');
  });
});
