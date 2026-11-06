import type {
  LoginView,
  Model,
  Project,
  ProviderView,
  PromptImage,
  PromptInfo,
  PromptText,
  Session,
  Settings
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
    ponytailLevel: string,
    model?: string
  ) =>
    request<Session>('/api/sessions', {
      method: 'POST',
      body: JSON.stringify({ projectId, title, ponytailLevel, model })
    }),
  updateSession: (id: string, title: string) =>
    request<Session>(`/api/sessions/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: JSON.stringify({ title })
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

  getSettings: () => request<Settings>('/api/settings'),
  updateSettings: (settings: Settings) =>
    request<Settings>('/api/settings', { method: 'PUT', body: JSON.stringify(settings) }),

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
  setOpenCodeKey: (key: string) =>
    request<{ ok: boolean }>('/api/providers/opencode-go/key', {
      method: 'PUT',
      body: JSON.stringify({ key })
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
    request<PromptInfo>(`/api/prompts/${encodeURIComponent(id)}`, { method: 'DELETE' })
};

export function eventsUrl(sessionId: string, after = 0): string {
  return `${base}/api/sessions/${encodeURIComponent(sessionId)}/events?after=${after}`;
}
