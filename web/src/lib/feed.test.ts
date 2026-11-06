import { describe, expect, it } from 'vitest';
import { applyEvent, emptyFeedState, type FeedState } from './feed';
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

  it('moves a queued steer to its delivery route', () => {
    const state = run(
      ev('steer_queued', { id: 'q1', text: 'hold on' }),
      ev('steer_delivered', { id: 'q1', route: 'injected' }, 2)
    );
    expect(state.items).toHaveLength(1);
    expect(state.items[0]).toMatchObject({ kind: 'steer', text: 'hold on', route: 'injected' });
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
