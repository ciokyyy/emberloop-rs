export type ComposerAction = 'prompt' | 'steer';

export function shouldSend(event: Pick<KeyboardEvent, 'key' | 'shiftKey'>): boolean {
  return event.key === 'Enter' && !event.shiftKey;
}

export function resolveComposerAction(working: boolean): ComposerAction {
  return working ? 'steer' : 'prompt';
}
