import { fireEvent, render, screen } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import FigcraftAgentHud from './FigcraftAgentHud.svelte';

it('keeps failed prompts and shows the error without leaving the editor', async () => {
  render(FigcraftAgentHud, {
    project: { id: 'p1', name: 'Design', root: '/project' },
    sessions: [{ id: 's1', projectId: 'p1', title: 'Designer', model: 'test', createdAt: 1, updatedAt: 1 }],
    onsendprompt: vi.fn().mockRejectedValue(new Error('Agent unavailable'))
  });
  await fireEvent.input(screen.getByRole('textbox'), { target: { value: 'Add a button' } });
  await fireEvent.submit(screen.getByRole('textbox').closest('form')!);
  expect((await screen.findByRole('alert')).textContent).toContain('Agent unavailable');
  expect((screen.getByRole('textbox') as HTMLInputElement).value).toBe('Add a button');
});
