export interface Project {
  id: string;
  name: string;
  root: string;
}

export interface Session {
  id: string;
  projectId: string;
  title: string;
  ponytailLevel: string;
  model: string;
  createdAt: number;
  updatedAt: number;
}

export interface Model {
  id: string;
  provider: string;
  label: string;
  kind: string;
  efforts: string[];
  image_input: boolean;
  context_window: number;
  source: string;
  stale: boolean;
  updated_at: number;
}

export interface Settings {
  proxy_compression: boolean;
  ponytail_default_level: string;
}

export interface PromptInfo {
  id: string;
  defaultHash: string;
  overrideHash: string | null;
  activeHash: string;
  differs: boolean;
  error: string | null;
}

export interface PromptText {
  id: string;
  text: string;
}

export interface PromptImage {
  mimeType: string;
  dataBase64: string;
}

export type SessionStatus = 'idle' | 'working' | 'failed' | 'paused';
export type SteerRoute = 'pending' | 'injected' | 'idle';

export type EventKind =
  | 'status'
  | 'prompt'
  | 'assistant_chunk'
  | 'tool_started'
  | 'tool_finished'
  | 'steer_queued'
  | 'steer_delivered'
  | 'notice'
  | 'compression'
  | 'usage'
  | 'error'
  | 'timing';

export interface SessionEvent {
  v: number;
  seq: number;
  sessionId: string;
  kind: EventKind;
  at: number;
  data: Record<string, unknown>;
}

export interface AccountView {
  id: string;
  label: string;
  tier: string;
  signedIn: boolean;
  lastLogin: number;
}

export interface ProviderView {
  id: string;
  name: string;
  signedIn: boolean;
  accounts: AccountView[];
}

export interface LoginView {
  id: string;
  state: 'running' | 'succeeded' | 'failed' | 'canceled';
  url?: string;
  output?: string;
  error?: string;
  tier?: string;
  accountId?: string;
}
