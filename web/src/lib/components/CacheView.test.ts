import { render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from '$lib/api';
import type { UsageRecord } from '$lib/api/types';
import CacheView from './CacheView.svelte';

const row = (seq: number, cached: number, creation: number, fresh: number): UsageRecord => {
  const prompt = cached + creation + fresh;
  return {
    seq, at: seq * 1000, inputTokens: prompt, freshInputTokens: fresh, cachedTokens: cached,
    cacheCreationTokens: creation, outputTokens: 20, hitPercent: prompt ? (cached / prompt) * 100 : null
  };
};

// The first request wrote the cache, the second read most of it back.
const fixture = [row(1, 0, 1000, 200), row(2, 900, 0, 100)];

afterEach(() => vi.restoreAllMocks());

describe('cache page', () => {
  it('shows the latest request and the token-weighted Session hit rate', async () => {
    vi.spyOn(api, 'sessionUsage').mockResolvedValue(fixture);
    render(CacheView, { sessionId: 's1' });
    expect((await screen.findByTestId('latest-hit')).textContent).toBe('90%');
    // 900 cached out of 2200 prompt tokens: 41%, not the 45% mean of the two requests.
    expect(screen.getByTestId('session-hit').textContent).toBe('41%');
    expect(screen.getByRole('img', { name: /prompt-cache hit rate per request/i })).toBeTruthy();
    expect(screen.getByRole('columnheader', { name: 'Written' })).toBeTruthy();
    expect(screen.getByRole('cell', { name: '#2' })).toBeTruthy();
  });

  it('explains an empty prompt-cache report instead of showing 0%', async () => {
    vi.spyOn(api, 'sessionUsage').mockResolvedValue([row(1, 0, 0, 0)]);
    render(CacheView, { sessionId: 's1' });
    expect(await screen.findByText(/no cache fields/i)).toBeTruthy();
    expect(screen.getByText(/minimum prompt size/i)).toBeTruthy();
    expect(screen.queryByTestId('latest-hit')).toBeNull();
  });

  it('says so when a Session has no requests yet', async () => {
    vi.spyOn(api, 'sessionUsage').mockResolvedValue([]);
    render(CacheView, { sessionId: 's1' });
    expect(await screen.findByText(/no model requests yet/i)).toBeTruthy();
  });

  it('shows loading until the history arrives', async () => {
    vi.spyOn(api, 'sessionUsage').mockReturnValue(new Promise(() => {}));
    render(CacheView, { sessionId: 's1' });
    expect(screen.getByText('Loading…')).toBeTruthy();
  });

  it('reports a failed load in an alert', async () => {
    vi.spyOn(api, 'sessionUsage').mockRejectedValue(new Error('500 boom'));
    render(CacheView, { sessionId: 's1' });
    expect((await screen.findByRole('alert')).textContent).toContain('500 boom');
  });

  it('reloads the history when the Session status changes, such as when a turn ends', async () => {
    const usage = vi.spyOn(api, 'sessionUsage').mockResolvedValue(fixture);
    const { rerender } = render(CacheView, { sessionId: 's1', status: 'working' });
    await screen.findByTestId('latest-hit');
    await rerender({ sessionId: 's1', status: 'idle' });
    await waitFor(() => expect(usage).toHaveBeenCalledTimes(2));
  });
});
