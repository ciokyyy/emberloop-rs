import { afterEach, expect, it, vi } from 'vitest';

const mocked = vi.hoisted(() => ({
  play: vi.fn(),
  unlock: vi.fn().mockResolvedValue(undefined)
}));

vi.mock('uisfx', () => ({
  createUISFX: () => ({ play: mocked.play, unlock: mocked.unlock })
}));

import { playAlert, reportVisible } from './notify';

afterEach(() => {
  reportVisible(null);
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function post() {
  const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

it('plays the error cue for a failed turn and the success cue otherwise', () => {
  playAlert(true);
  playAlert(false);
  expect(mocked.play.mock.calls.map(([cue]) => cue)).toEqual(['error', 'success']);
});

it('reports the visible Session every twenty seconds and stops when it closes', async () => {
  vi.useFakeTimers();
  const fetchMock = post();
  reportVisible('session-1');
  expect(fetchMock).toHaveBeenCalledTimes(1);
  expect(fetchMock.mock.calls[0][0]).toContain('/api/push/visible');
  expect(JSON.parse(String(fetchMock.mock.calls[0][1]?.body))).toEqual({ sessionId: 'session-1' });

  await vi.advanceTimersByTimeAsync(20_000);
  expect(fetchMock).toHaveBeenCalledTimes(2);

  reportVisible(null);
  await vi.advanceTimersByTimeAsync(60_000);
  expect(fetchMock).toHaveBeenCalledTimes(2);
});

it('says nothing while the tab is hidden', async () => {
  vi.useFakeTimers();
  const fetchMock = post();
  reportVisible('session-1');
  Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
  await vi.advanceTimersByTimeAsync(40_000);
  expect(fetchMock).toHaveBeenCalledTimes(1);
  Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
});
