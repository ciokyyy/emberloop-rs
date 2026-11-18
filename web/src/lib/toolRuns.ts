import type { FeedItem } from './feed';

/** One tool call (or thinking span) in the shape the Go tool feed model reads. */
export interface ToolRun {
  name: string;
  input: string;
  output: string;
  status: 'running' | 'completed' | 'failed';
  elapsedMs: number;
  display?: Record<string, unknown>;
  thinking?: boolean;
  live?: boolean;
  target?: string;
  children?: ToolRun[];
}

export type Segment =
  | { type: 'item'; item: FeedItem }
  | { type: 'tools'; id: string; runs: ToolRun[]; running: boolean };

function displayChildrenOf(display?: Record<string, unknown>): ToolRun[] | undefined {
  const raw = display?.children;
  if (!Array.isArray(raw) || raw.length === 0) return undefined;
  return raw
    .filter((c): c is Record<string, unknown> => Boolean(c && typeof c === 'object'))
    .map((c) => {
      const target = typeof c.target === 'string' ? c.target : '';
      return {
        name: typeof c.name === 'string' ? c.name : 'tool',
        input: typeof c.input === 'string' ? c.input : target ? JSON.stringify({ target, path: target, command: target, pattern: target, query: target }) : '',
        output: typeof c.output === 'string' ? c.output : '',
        status: c.status === 'failed' ? 'failed' : 'completed',
        elapsedMs: typeof c.ms === 'number' ? c.ms : typeof c.elapsedMs === 'number' ? c.elapsedMs : 0,
        target,
        display: c.display && typeof c.display === 'object' ? (c.display as Record<string, unknown>) : undefined,
      };
    });
}

export function runOf(item: FeedItem): ToolRun {
  if (item.kind === 'thinking') {
    return {
      name: 'thinking',
      input: '',
      output: item.text ?? '',
      status: item.live ? 'running' : 'completed',
      elapsedMs: item.elapsedMs ?? 0,
      thinking: true,
      live: item.live,
    };
  }
  const children = displayChildrenOf(item.display);
  return {
    name: item.tool || item.name || '',
    input: item.input ?? '',
    output: item.output ?? item.body ?? '',
    status: item.status ?? (item.finished ? 'completed' : 'running'),
    elapsedMs: item.elapsedMs ?? 0,
    display: item.display,
    ...(children ? { children } : {}),
  };
}

/** Consecutive tool calls and thinking spans fold into one block; messages break it. */
export function segments(items: FeedItem[]): Segment[] {
  const out: Segment[] = [];
  for (const item of items) {
    if (item.kind !== 'tool' && item.kind !== 'thinking') {
      out.push({ type: 'item', item });
      continue;
    }
    const last = out.at(-1);
    const run = runOf(item);
    if (last?.type === 'tools') {
      last.runs.push(run);
      last.running ||= run.status === 'running' || !!item.live;
    } else {
      out.push({ type: 'tools', id: item.id, runs: [run], running: run.status === 'running' || !!item.live });
    }
  }
  return out;
}
