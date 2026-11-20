export type ComposerAction = 'prompt' | 'steer';

export function shouldSend(event: Pick<KeyboardEvent, 'key' | 'shiftKey'>): boolean {
  return event.key === 'Enter' && !event.shiftKey;
}

/** The skill name being typed after a leading slash, while the cursor is still inside that word. */
export function slashQuery(draft: string, cursor: number): string | null {
  if (!draft.startsWith('/')) return null;
  const word = draft.slice(1).split(/\s/, 1)[0] ?? '';
  return cursor <= 1 + word.length ? word : null;
}

const SLASH = /^\/([A-Za-z0-9][\w.-]*)(?:\s+([\s\S]*))?$/;

/** A draft that starts with a known skill name becomes the read instruction and the rest, as typed. */
export function expandSlash(draft: string, knownNames: Iterable<string>): string {
  const match = SLASH.exec(draft.trim());
  if (!match || !new Set(knownNames).has(match[1])) return draft;
  const read = `Read \`el://skills/${match[1]}\` and follow it.`;
  const rest = (match[2] ?? '').trim();
  return rest ? `${read}\n\n${rest}` : read;
}

export function resolveComposerAction(working: boolean): ComposerAction {
  return working ? 'steer' : 'prompt';
}

/** A one-shot Goldfish shows no composer: its Session takes no second prompt. */
export function isOneShot(role?: string | null): boolean {
  return role === 'goldfish';
}
