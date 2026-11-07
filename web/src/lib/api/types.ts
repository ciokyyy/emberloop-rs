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
  thoughtLevel?: string;
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
  title_generation: boolean;
  title_model: string;
  proxy_compression: boolean;
  ponytail_default_level: string;
  todo_reminders: boolean;
  todo_reminders_max: number;
  todo_auto_expand: boolean;
  todo_hide_when_done: boolean;
  max_context_limit: number;
  min_pressure_benefit_tokens: number;
  emergency_threshold_percent: number;
  preserve_recent_messages: number;
  protected_tools: string[];
  inject_tool: boolean;
  inject_nudge: boolean;
  tier2_trigger: number;
  tier3_trigger: number;
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
export type SteerRoute = 'pending' | 'injected' | 'idle' | 'cap';

export type EventKind =
  | 'session_title'
  | 'status'
  | 'prompt'
  | 'assistant_chunk'
  | 'thinking_chunk'
  | 'tool_started'
  | 'tool_finished'
  | 'steer_queued'
  | 'steer_delivered'
  | 'notice'
  | 'todos'
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
  /** `login` (browser flow) or `key` (pasted API key). */
  auth: string;
  accounts: AccountView[];
}

export interface LoginView {
  id: string;
  state: 'running' | 'succeeded' | 'failed' | 'canceled';
  url?: string;
  userCode?: string;
  verificationUri?: string;
  expiresAt?: number;
  output?: string;
  error?: string;
  tier?: string;
  accountId?: string;
}

/** Estimated text tokens in the final outgoing request; image bytes are never tokenized as text. */
export interface ContextBreakdown {
  system: number;
  toolDefinitions: number;
  toolCalls: number;
  toolResults: number;
  compression: number;
  conversation: number;
  images: number;
  imageCount: number;
  estimatedTokens: number;
}

export interface UsageView {
  windows: { label: string; usedPercent: number; resetsAt: string | null }[];
  balance: { currency: string; remaining: number } | null;
  error: string | null;
}

/** A tool key Settings stores in the runtime env file the shells source. */
export interface ToolKey {
  name: string;
  variable: string;
  set: boolean;
}
