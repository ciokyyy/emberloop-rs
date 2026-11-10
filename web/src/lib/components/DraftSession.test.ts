import { fireEvent, render, screen } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import DraftSession from './DraftSession.svelte';

it('keeps the greeting central and sends the selected model effort from the bottom controls', async () => {
  const onsubmit = vi.fn();
  render(DraftSession, { sessionId: 'draft:test', model: 'claude/opus', models: [
    { id: 'claude/opus', label: 'Opus', provider: 'claude', source: 'claude', kind: 'chat', efforts: ['low', 'max'], image_input: true, context_window: 1000000, stale: false, updated_at: 0 }
  ], onsubmit });
  expect(screen.getByRole('heading', { name: 'What are we working on today?' })).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'Choose model and reasoning effort' }));
  await fireEvent.click(screen.getByRole('button', { name: 'max' }));
  await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
  await fireEvent.input(screen.getByRole('textbox'), { target: { value: 'hello' } });
  await fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  expect(onsubmit).toHaveBeenCalledWith('hello', [], 'max');
});
