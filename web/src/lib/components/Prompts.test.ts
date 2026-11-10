import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { api } from '$lib/api';
import Prompts from './Prompts.svelte';

afterEach(() => vi.restoreAllMocks());

const info = {
  id: 'tool.edit.description',
  defaultHash: 'a',
  overrideHash: null,
  activeHash: 'a',
  differs: false,
  error: null
};
const proposal = {
  id: 'tool.edit.description',
  text: 'Proposed edit text',
  reason: 'Ranges ate lines',
  current: 'Current edit text',
  stale: true
};

it('shows a proposal beside the active text and approves it', async () => {
  vi.spyOn(api, 'listPrompts').mockResolvedValue([info]);
  const list = vi.spyOn(api, 'listPromptProposals');
  list.mockResolvedValueOnce([proposal]).mockResolvedValue([]);
  const approve = vi.spyOn(api, 'approvePromptProposal').mockResolvedValue(info);
  render(Prompts);

  expect(await screen.findByText('Ranges ate lines')).toBeTruthy();
  expect(screen.getByText('Proposed edit text')).toBeTruthy();
  expect(screen.getByText('Current edit text')).toBeTruthy();
  expect(screen.getByText('stale')).toBeTruthy();

  await fireEvent.click(screen.getByRole('button', { name: 'Approve' }));
  expect(approve).toHaveBeenCalledWith('tool.edit.description');
  await waitFor(() => expect(screen.queryByText('Ranges ate lines')).toBeNull());
  expect(list).toHaveBeenCalledTimes(2);
});

it('rejects a proposal', async () => {
  vi.spyOn(api, 'listPrompts').mockResolvedValue([info]);
  vi.spyOn(api, 'listPromptProposals').mockResolvedValueOnce([proposal]).mockResolvedValue([]);
  const discard = vi.spyOn(api, 'discardPromptProposal').mockResolvedValue(undefined);
  render(Prompts);

  await fireEvent.click(await screen.findByRole('button', { name: 'Reject' }));
  expect(discard).toHaveBeenCalledWith('tool.edit.description');
});
