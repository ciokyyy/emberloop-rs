import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { api } from '$lib/api';
import Providers from './Providers.svelte';

afterEach(() => vi.restoreAllMocks());

it('renders a login flow inside its provider card instead of a new card', async () => {
  vi.spyOn(api, 'listProviders').mockResolvedValue([
    { id: 'openai-codex', name: 'ChatGPT', signedIn: false, auth: 'login', accounts: [] }
  ]);
  vi.spyOn(api, 'startCodexLogin').mockResolvedValue({
    id: 'l1',
    state: 'running',
    userCode: 'ABCD-1234',
    verificationUri: 'https://auth.example/device'
  });
  render(Providers, { onmodels: () => {} });
  await fireEvent.click(await screen.findByRole('button', { name: 'Connect' }));
  expect(await screen.findByText('ABCD-1234')).toBeTruthy();
  expect(screen.getByText('Waiting for sign-in')).toBeTruthy();
  expect(screen.getAllByText('ChatGPT')).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Connect' })).toBeNull();
});
