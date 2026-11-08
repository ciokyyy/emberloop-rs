import type {
  UsageRecord,
  UsageView,
  LoginView,
  Model,
  Project,
  ProviderView,
  PromptImage,
  PromptInfo,
  PromptText,
  Session,
  SessionEvent,
  Settings,
  ToolKey
} from './types';

const base = String(import.meta.env.VITE_API_BASE ?? '').replace(/\/$/, '');

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(base + path, {
    ...init,
    headers: { 'content-type': 'application/json', ...init.headers }
  });
  if (!response.ok) {
    const raw = await response.text();
    const detail = raw.replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ').trim().slice(0, 200);
    throw new Error(`${response.status} ${detail}`.trim());
  }
  if (!(response.headers.get('content-type') ?? '').includes('application/json')) {
    return undefined as T;
  }
  return (await response.json()) as T;
}

export const api = {
  listProjects: () => request<Project[]>('/api/projects'),
  createProject: (name: string, root: string) =>
    request<Project>('/api/projects', { method: 'POST', body: JSON.stringify({ name, root }) }),
  updateProject: (id: string, patch: { name?: string; root?: string }) =>
    request<Project>(`/api/projects/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: JSON.stringify(patch)
    }),
  deleteProject: (id: string) =>
    request<Project>(`/api/projects/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  listSessions: (projectId?: string) =>
    request<Session[]>(
      `/api/sessions${projectId ? `?projectId=${encodeURIComponent(projectId)}` : ''}`
    ),
  createSession: (
    projectId: string,
    title: string | undefined,
    model?: string,
    thoughtLevel?: string
  ) =>
    request<Session>('/api/sessions', {
      method: 'POST',
      body: JSON.stringify({ projectId, title, model, thoughtLevel })
    }),
  updateSession: (id: string, patch: { title?: string; model?: string; thoughtLevel?: string }) =>
    request<Session>(`/api/sessions/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: JSON.stringify(patch)
    }),
  deleteSession: (id: string) =>
    request<Session>(`/api/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  prompt: (id: string, text: string, images: PromptImage[]) =>
    request<void>(`/api/sessions/${encodeURIComponent(id)}/prompt`, {
      method: 'POST',
      body: JSON.stringify({ text, images })
    }),
  steer: (id: string, text: string) =>
    request<void>(`/api/sessions/${encodeURIComponent(id)}/steer`, {
      method: 'POST',
      body: JSON.stringify({ text })
    }),
  cancel: (id: string) =>
    request<void>(`/api/sessions/${encodeURIComponent(id)}/cancel`, { method: 'POST' }),

  listModels: () => request<Model[]>('/api/models'),
  refreshModels: () => request<Model[]>('/api/models/refresh', { method: 'POST' }),

  listToolKeys: () => request<ToolKey[]>('/api/tool-keys'),
  // Raw audio body; the content type overrides the JSON default in request().
  transcribe: (audio: Blob) =>
    request<{ text: string }>('/api/transcribe', {
      method: 'POST',
      headers: { 'content-type': audio.type || 'audio/webm' },
      body: audio
    }),
  setToolKey: (name: string, value: string) =>
    request<ToolKey>(`/api/tool-keys/${encodeURIComponent(name)}`, {
      method: 'PUT',
      body: JSON.stringify({ value })
    }),

  getSettings: () => request<Settings>('/api/settings'),
  updateSettings: (settings: Settings) =>
    request<Settings>('/api/settings', { method: 'PUT', body: JSON.stringify(settings) }),

  providerUsage: (provider: string, refresh = false): Promise<UsageView> =>
    request(`/api/providers/${encodeURIComponent(provider)}/usage?refresh=${refresh}`),
  /** The newest `limit` model requests of a Session, oldest first; the server default is 200. */
  sessionUsage: (id: string, limit?: number) =>
    request<UsageRecord[]>(
      `/api/sessions/${encodeURIComponent(id)}/usage${limit ? `?limit=${limit}` : ''}`
    ),
  listProviders: () => request<ProviderView[]>('/api/providers'),
  startAgyLogin: (label: string) =>
    request<LoginView>('/api/providers/agy/login', {
      method: 'POST',
      body: JSON.stringify({ label })
    }),
  getAgyLogin: (id: string) =>
    request<LoginView>(`/api/providers/agy/login/${encodeURIComponent(id)}`),
  cancelAgyLogin: (id: string) =>
    request<{ ok: boolean }>(`/api/providers/agy/login/${encodeURIComponent(id)}`, {
      method: 'DELETE'
    }),
  completeAgyLogin: (id: string, address: string) =>
    request<LoginView>(`/api/providers/agy/login/${encodeURIComponent(id)}/complete`, {
      method: 'POST',
      body: JSON.stringify({ address })
    }),
  signOutAccount: (id: string) =>
    request<{ ok: boolean }>(`/api/providers/agy/accounts/${encodeURIComponent(id)}`, {
      method: 'DELETE'
    }),
  setProviderKey: (id: string, key: string) =>
    request<{ ok: boolean }>(`/api/providers/${encodeURIComponent(id)}/key`, {
      method: 'PUT',
      body: JSON.stringify({ key })
    }),
  startCodexLogin: (label = '') =>
    request<LoginView>('/api/providers/openai-codex/login', {
      method: 'POST',
      body: JSON.stringify({ label })
    }),
  getCodexLogin: (id: string) =>
    request<LoginView>(`/api/providers/openai-codex/login/${encodeURIComponent(id)}`),
  startClaudeLogin: (label = '') =>
    request<LoginView>('/api/providers/claude/login', {
      method: 'POST',
      body: JSON.stringify({ label })
    }),
  getClaudeLogin: (id: string) =>
    request<LoginView>(`/api/providers/claude/login/${encodeURIComponent(id)}`),
  cancelClaudeLogin: (id: string) =>
    request<{ ok: boolean }>(`/api/providers/claude/login/${encodeURIComponent(id)}`, {
      method: 'DELETE'
    }),
  completeClaudeLogin: (id: string, code: string) =>
    request<LoginView>(`/api/providers/claude/login/${encodeURIComponent(id)}/complete`, {
      method: 'POST',
      body: JSON.stringify({ code })
    }),
  cancelCodexLogin: (id: string) =>
    request<{ ok: boolean }>(`/api/providers/openai-codex/login/${encodeURIComponent(id)}`, {
      method: 'DELETE'
    }),
  listPrompts: () => request<PromptInfo[]>('/api/prompts'),
  getPrompt: (id: string) =>
    request<PromptText>(`/api/prompts/${encodeURIComponent(id)}`),
  updatePrompt: (id: string, text: string) =>
    request<PromptInfo>(`/api/prompts/${encodeURIComponent(id)}`, {
      method: 'PUT',
      body: JSON.stringify({ text })
    }),
  deletePrompt: (id: string) =>
    request<PromptInfo>(`/api/prompts/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  // history is one page older than `before`, oldest first: the client walks back from the tail
  // window one page at a time, so a refresh never pays for the whole Session.
  history: (id: string, before: number, limit = 8) =>
    request<SessionEvent[]>(
      `/api/sessions/${encodeURIComponent(id)}/history?before=${before}&limit=${limit}`
    )
};

export function eventsUrl(sessionId: string, after = 0, limit = 0): string {
  const window = limit > 0 ? `&limit=${limit}` : '';
  return `${base}/api/sessions/${encodeURIComponent(sessionId)}/events?after=${after}${window}`;
}
