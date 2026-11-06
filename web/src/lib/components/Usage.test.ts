import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import { api } from '$lib/api';
import Usage from './Usage.svelte';

it('shows real windows and balances and refreshes without changing credentials', async () => {
  vi.spyOn(api, 'listProviders').mockResolvedValue([
    { id: 'openai-codex', name: 'ChatGPT', signedIn: true, auth: 'login', accounts: [] },
    { id: 'openrouter', name: 'OpenRouter', signedIn: true, auth: 'key', accounts: [] }
  ]);
  const usage = vi.spyOn(api, 'providerUsage').mockImplementation(async (id) => id === 'openrouter'
    ? { windows: [], balance: { currency: 'USD', remaining: 12.34 }, error: null }
    : { windows: [{ label: '5-hour', usedPercent: 42, resetsAt: null }], balance: null, error: null });
  render(Usage);
  expect(await screen.findByText('42% used')).toBeTruthy();
  expect(screen.getByText('USD 12.34 remaining')).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(usage).toHaveBeenCalledWith('openrouter', true));
  vi.restoreAllMocks();
});
