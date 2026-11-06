import { describe, expect, it } from 'vitest';
import { applyEvent, applySnapshot, emptyFeedState } from './feed';
import type { SessionEvent } from './api/types';
const breakdown = { system: 12, toolDefinitions: 2, toolCalls: 3, toolResults: 4, compression: 5, conversation: 6, images: 0, imageCount: 1, estimatedTokens: 32 };
const data = { inputTokens: 100, contextWindow: 1000, breakdown };
const event: SessionEvent = { v: 1, seq: 1, sessionId: 's', at: 0, kind: 'usage', data };
describe('context breakdown replay', () => {
  it('keeps the proxy breakdown on live usage and snapshot reload', () => {
    expect(applyEvent(emptyFeedState(), event).usage?.breakdown).toEqual(breakdown);
    expect(applySnapshot(emptyFeedState(), { usage: data }).usage?.breakdown).toEqual(breakdown);
  });
  it('does not reuse an old breakdown on a newer usage event without one', () => {
    const state = applyEvent(emptyFeedState(), event);
    expect(applyEvent(state, { ...event, data: { inputTokens: 50 } }).usage?.breakdown).toBeUndefined();
  });
});
