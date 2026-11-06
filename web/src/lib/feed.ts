import type { SessionEvent, SessionStatus, SteerRoute } from './api/types';

export type ToolStatus = 'running' | 'completed' | 'failed';

export interface FeedItem {
  id: string;
  kind: 'user' | 'assistant' | 'thinking' | 'tool' | 'compression' | 'steer' | 'notice' | 'error';
  text?: string;
  live?: boolean;
  imagesCount?: number;
  /** Short tool name without the MCP prefix (`read`, `bash`). */
  tool?: string;
  /** Raw tool name as the harness sent it; the tool feed derives its family from it. */
  name?: string;
  /** Tool arguments as a JSON string. */
  input?: string;
  output?: string;
  status?: ToolStatus;
  display?: Record<string, unknown>;
  startedAt?: number;
  elapsedMs?: number;
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
const isLive = (item?: FeedItem) => !!item && (item.kind === 'assistant' || item.kind === 'thinking') && !!item.live;

function closeLive(items: FeedItem[]): FeedItem[] {
  const last = items.at(-1);
  if (!last || !isLive(last)) return items;
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

function toolInputText(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value && typeof value === 'object' && Object.keys(value).length > 0) return JSON.stringify(value);
  return '';
}

/** A tool result arrives as plain text (pi) or as `{ text, content[] }` (agy). */
export function outputText(value: unknown): string {
  if (typeof value === 'string') return value;
  if (!value || typeof value !== 'object') return '';
  const record = value as Record<string, unknown>;
  if (typeof record.text === 'string') return record.text;
  if (Array.isArray(record.content)) {
    return record.content
      .map((part) => (part && typeof part === 'object' ? text((part as Record<string, unknown>).text) : ''))
      .filter(Boolean)
      .join('\n');
  }
  return '';
}

function toolSummary(data: Record<string, unknown>): string {
  const input = (data.input ?? {}) as Record<string, unknown>;
  return text(data.summary ?? data.path ?? input.command ?? input.path ?? input.pattern);
}

function toolId(data: Record<string, unknown>): string {
  return text(data.id ?? data.callId ?? data.toolCallId);
}

function displayOf(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === 'object' ? (value as Record<string, unknown>) : undefined;
}

function statusFromFinish(data: Record<string, unknown>): ToolStatus {
  const status = text(data.status).toLowerCase();
  return status === 'failed' || status === 'error' ? 'failed' : 'completed';
}

/** The harness repeats `tool_started` as the arguments stream in; the row keeps the fullest copy. */
function startTool(items: FeedItem[], event: SessionEvent): FeedItem[] {
  const data = event.data;
  const id = toolId(data);
  const input = toolInputText(data.input);
  const name = text(data.name ?? data.tool);
  const index = id ? items.findLastIndex((item) => item.kind === 'tool' && item.id === id) : -1;
  if (index !== -1) {
    const next = [...items];
    const row = next[index];
    next[index] = {
      ...row,
      name: name || row.name,
      tool: toolName(data) || row.tool,
      input: input || row.input,
      summary: toolSummary(data) || row.summary,
      display: displayOf(data.display) ?? row.display
    };
    return next;
  }
  return [
    ...closeLive(items),
    {
      id: id || nextId('tool'),
      kind: 'tool',
      tool: toolName(data),
      name,
      input,
      summary: toolSummary(data),
      meta: text(data.meta),
      body: text(data.body),
      display: displayOf(data.display),
      status: 'running',
      startedAt: event.at,
      finished: false
    }
  ];
}

function finishTool(items: FeedItem[], event: SessionEvent): FeedItem[] {
  const id = toolId(event.data);
  const tool = toolName(event.data);
  const index = items.findLastIndex(
    (item) => item.kind === 'tool' && !item.finished && (id ? item.id === id : item.tool === tool)
  );
  const output = outputText(event.data.output ?? event.data.body ?? event.data.diff);
  const meta = text(event.data.meta);
  const status = statusFromFinish(event.data);
  const display = displayOf(event.data.display);
  const input = toolInputText(event.data.input);
  if (index === -1) {
    return [
      ...items,
      {
        id: id || nextId('tool'),
        kind: 'tool',
        tool,
        name: text(event.data.name ?? event.data.tool),
        input,
        summary: toolSummary(event.data) || tool,
        meta,
        body: output,
        output,
        display,
        status,
        finished: true
      }
    ];
  }
  const next = [...items];
  const row = next[index];
  next[index] = {
    ...row,
    name: text(event.data.name) || row.name,
    tool: tool || row.tool,
    input: input || row.input,
    summary: row.summary || toolSummary(event.data),
    meta: meta || row.meta,
    body: output || row.body,
    output: output || row.output,
    display: display ?? row.display,
    status,
    elapsedMs: row.startedAt && event.at ? Math.max(0, event.at - row.startedAt) : row.elapsedMs,
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

function appendChunk(state: FeedState, kind: 'assistant' | 'thinking', chunk: string): FeedState {
  const last = state.items.at(-1);
  if (last?.kind === kind && last.live) {
    return {
      ...state,
      items: [...state.items.slice(0, -1), { ...last, text: (last.text ?? '') + chunk }]
    };
  }
  return {
    ...state,
    items: [...closeLive(state.items), { id: nextId(kind), kind, text: chunk, live: true }]
  };
}

export function applyEvent(state: FeedState, event: SessionEvent): FeedState {
  const data = event.data ?? {};
  switch (event.kind) {
    case 'status': {
      const status = statusOf(data.state ?? data.status);
      return { ...state, status, items: status === 'working' ? state.items : closeLive(state.items) };
    }

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

    case 'assistant_chunk':
      return appendChunk(state, 'assistant', text(data.text));

    case 'thinking_chunk':
      return appendChunk(state, 'thinking', text(data.text));

    case 'tool_started':
      return { ...state, items: startTool(state.items, event) };

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
          window: number(data.contextWindow ?? data.window) || state.usage?.window || 0
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
