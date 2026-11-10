import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import { api } from '$lib/api';
import type { UsageRecord } from '$lib/api/types';
import ContextDetails from './ContextDetails.svelte';

it('opens the Session cache page from the context details', async () => {
  const oncache = vi.fn();
  render(ContextDetails, { used: 1000, size: 200000, oncache });
  await fireEvent.click(screen.getByRole('button', { name: 'Prompt cache' }));
  expect(oncache).toHaveBeenCalledOnce();
});

// 900 cached out of a 2200-token prompt: the ring reads the Session hit rate, not the latest request.
it('shows the Session cache hit rate in the ring', async () => {
  const row = (seq: number, cached: number, creation: number, fresh: number): UsageRecord => ({
    seq, at: seq * 1000, inputTokens: cached + creation + fresh, freshInputTokens: fresh,
    cachedTokens: cached, cacheCreationTokens: creation, outputTokens: 20, hitPercent: null, fold: false
  });
  vi.spyOn(api, 'sessionUsage').mockResolvedValue([row(1, 0, 1000, 200), row(2, 900, 0, 100)]);
  render(ContextDetails, { sessionId: 's1', used: 1000, size: 200000 });
  await waitFor(() => expect(screen.getByTestId('cache-ring').getAttribute('aria-label')).toBe('Prompt cache hit 41%'));
});
