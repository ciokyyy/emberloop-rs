import type { SessionEvent, SessionStatus, SteerRoute } from './api/types';

export interface FeedItem {
  id: string;
  kind: 'user' | 'assistant' | 'tool' | 'compression' | 'steer' | 'notice' | 'error';
  text?: string;
  live?: boolean;
  imagesCount?: number;
  tool?: string;
  summary?: string;
  meta?: string;
  body?: string;
  finished?: boolean;
  route?: SteerRoute;
}

export interface FeedState {
  items: FeedItem[];
  status: SessionStatus;
  usage: { used: number; window: number } | null;
}

export function emptyFeedState(): FeedState {
  return { items: [], status: 'idle', usage: null };
}

let counter = 0;
const nextId = (prefix: string) => `${prefix}-${++counter}`;

const text = (value: unknown): string => (typeof value === 'string' ? value : '');
const number = (value: unknown): number => (typeof value === 'number' && Number.isFinite(value) ? value : 0);
const route = (value: unknown): SteerRoute | undefined =>
  value === 'pending' || value === 'injected' || value === 'idle' ? value : undefined;

function closeLive(items: FeedItem[]): FeedItem[] {
  const last = items.at(-1);
  if (!last || last.kind !== 'assistant' || !last.live) return items;
  return [...items.slice(0, -1), { ...last, live: false }];
}

function statusOf(value: unknown): SessionStatus {
  if (value === 'working') return 'working';
  if (value === 'failed') return 'failed';
  if (value === 'paused') return 'paused';
  return 'idle';
}

/** agy reports MCP tools as `el_<name>`, pi as `mcp__el__<name>`; the feed knows them by `<name>`. */
function toolName(data: Record<string, unknown>): string {
  return text(data.name ?? data.tool).replace(/^(?:mcp__)?el_+/, '');
}

function toolSummary(data: Record<string, unknown>): string {
  const input = (data.input ?? {}) as Record<string, unknown>;
  return text(data.summary ?? data.path ?? input.command ?? input.path ?? input.pattern);
}

function toolId(data: Record<string, unknown>): string {
  return text(data.id ?? data.callId ?? data.toolCallId);
}

function finishTool(items: FeedItem[], event: SessionEvent): FeedItem[] {
  const id = toolId(event.data);
  const name = toolName(event.data);
  const index = items.findLastIndex(
    (item) =>
      item.kind === 'tool' &&
      !item.finished &&
      (id ? item.id === id : item.tool === name)
  );
  const body = text(event.data.output ?? event.data.body ?? event.data.diff);
  const meta = text(event.data.meta);
  if (index === -1) {
    return [
      ...items,
      {
        id: id || nextId('tool'),
        kind: 'tool',
        tool: name,
        summary: toolSummary(event.data) || name,
        meta,
        body,
        finished: true
      }
    ];
  }
  const next = [...items];
  next[index] = {
    ...next[index],
    meta: meta || next[index].meta,
    body: body || next[index].body,
    finished: true
  };
  return next;
}

function deliver(items: FeedItem[], event: SessionEvent, kind: 'steer' | 'notice'): FeedItem[] {
  const id = toolId(event.data);
  const index = items.findLastIndex(
    (item) => item.kind === kind && item.route === 'pending' && (!id || item.id === id)
  );
  const routeValue = route(event.data.route) ?? 'injected';
  if (index === -1) {
    return [
      ...items,
      {
        id: id || nextId(kind),
        kind,
        text: text(event.data.text),
        route: routeValue
      }
    ];
  }
  const next = [...items];
  next[index] = { ...next[index], route: routeValue };
  return next;
}

export function applyEvent(state: FeedState, event: SessionEvent): FeedState {
  const data = event.data ?? {};
  switch (event.kind) {
    case 'status':
      return { ...state, status: statusOf(data.state ?? data.status) };

    case 'prompt':
      return {
        ...state,
        items: [
          ...closeLive(state.items),
          {
            id: nextId('user'),
            kind: 'user',
            text: text(data.text),
            imagesCount: number(data.imagesCount) || number(data.images)
          }
        ]
      };

    case 'assistant_chunk': {
      const chunk = text(data.text);
      const last = state.items.at(-1);
      if (last?.kind === 'assistant' && last.live) {
        return {
          ...state,
          items: [...state.items.slice(0, -1), { ...last, text: (last.text ?? '') + chunk }]
        };
      }
      return {
        ...state,
        items: [...state.items, { id: nextId('assistant'), kind: 'assistant', text: chunk, live: true }]
      };
    }

    case 'tool_started':
      return {
        ...state,
        items: [
          ...closeLive(state.items),
          {
            id: toolId(data) || nextId('tool'),
            kind: 'tool',
            tool: toolName(data),
            summary: toolSummary(data),
            meta: text(data.meta),
            body: text(data.body),
            finished: false
          }
        ]
      };

    case 'tool_finished':
      return { ...state, items: closeLive(finishTool(state.items, event)) };

    case 'steer_queued':
      return {
        ...state,
        items: [
          ...state.items,
          { id: toolId(data) || nextId('steer'), kind: 'steer', text: text(data.text), route: 'pending' }
        ]
      };

    case 'steer_delivered':
      return { ...state, items: deliver(state.items, event, 'steer') };

    case 'notice':
      if (data.route === 'pending') {
        return {
          ...state,
          items: [
            ...state.items,
            { id: toolId(data) || nextId('notice'), kind: 'notice', text: text(data.text), route: 'pending' }
          ]
        };
      }
      return { ...state, items: deliver(state.items, event, 'notice') };

    case 'compression':
      return {
        ...state,
        items: [
          ...state.items,
          {
            id: nextId('compression'),
            kind: 'compression',
            text: text(data.text ?? data.summary)
          }
        ]
      };

    case 'usage':
      return {
        ...state,
        usage: {
          used: number(data.inputTokens ?? data.used ?? data.tokens),
          window: number(data.contextWindow ?? data.window) || 1_000_000
        }
      };

    case 'error':
      return {
        ...state,
        items: [
          ...state.items,
          { id: nextId('error'), kind: 'error', text: text(data.message ?? data.text) }
        ]
      };

    default:
      return state;
  }
}
