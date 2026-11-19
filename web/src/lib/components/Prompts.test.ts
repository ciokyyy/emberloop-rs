import { fireEvent, render, screen } from '@testing-library/svelte';
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

it('opens a prompt and shows its active text', async () => {
  vi.spyOn(api, 'listPrompts').mockResolvedValue([info]);
  vi.spyOn(api, 'getPrompt').mockResolvedValue({ id: info.id, text: 'Active edit text' });
  render(Prompts);

  await fireEvent.click(await screen.findByRole('button', { name: /tool\.edit\.description/ }));
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('Active edit text');
});
