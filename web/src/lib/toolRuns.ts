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
  children?: ToolRun[];
}

export type Segment =
  | { type: 'item'; item: FeedItem }
  | { type: 'tools'; id: string; runs: ToolRun[]; running: boolean };

export function runOf(item: FeedItem): ToolRun {
  if (item.kind === 'thinking') {
    return { name: 'thinking', input: '', output: item.text ?? '', status: 'completed', elapsedMs: 0, thinking: true };
  }
  return {
    name: item.tool || item.name || '',
    input: item.input ?? '',
    output: item.output ?? item.body ?? '',
    status: item.status ?? (item.finished ? 'completed' : 'running'),
    elapsedMs: item.elapsedMs ?? 0,
    display: item.display
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
