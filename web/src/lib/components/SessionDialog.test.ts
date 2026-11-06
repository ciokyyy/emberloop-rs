import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { Model } from '$lib/api/types';
import SessionDialog from './SessionDialog.svelte';

const model = (id: string, efforts: string[], image_input = true): Model => ({
  id, label: id, provider: 'pi', kind: 'chat', efforts, image_input,
  context_window: 200000, source: 'pi', stale: false, updated_at: 0
});
const models = [model('one', ['low', 'high']), model('two', ['off', 'xhigh', 'max'], false)];
const props = { open: true, models, model: 'one', thoughtLevel: 'high', onapply: vi.fn() };

describe('Session dialog', () => {
  it('changes reasoning choices with the selected model and submits only a supported effort', async () => {
    const onapply = vi.fn();
    render(SessionDialog, { ...props, onapply });
    await fireEvent.click(screen.getByRole('button', { name: 'Show model options' }));
    await fireEvent.pointerUp(screen.getByRole('option', { name: /two/ }));
    await waitFor(() => expect(screen.queryByRole('option', { name: 'high' })).toBeNull());
    await fireEvent.change(screen.getByLabelText('Reasoning effort'), { target: { value: 'max' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
    expect(onapply).toHaveBeenCalledWith({ model: 'two', thoughtLevel: 'max' });
  });

  it('shows provider totals separately from the estimated context categories', async () => {
    render(SessionDialog, { ...props, tab: 'context', used: 12000, size: 200000,
      breakdown: { system: 100, toolDefinitions: 200, toolCalls: 300, toolResults: 400,
        compression: 500, conversation: 600, images: 0, estimatedTokens: 2100, imageCount: 2 } });
    expect(await screen.findByText('System prompts')).toBeTruthy();
    expect(screen.getByText('Tool definitions')).toBeTruthy();
    expect(screen.getByText('Tool results')).toBeTruthy();
    expect(screen.getByText('Compression summaries')).toBeTruthy();
    expect(screen.getByText(/Provider-reported/)).toBeTruthy();
    expect(screen.getByText(/image token cost is not available/i)).toBeTruthy();
  });

  it('keeps the dialog and the error visible when apply fails', async () => {
    render(SessionDialog, { ...props, onapply: vi.fn().mockRejectedValue(new Error('Model unavailable')) });
    await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
    expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'Model unavailable');
    expect(screen.getByRole('dialog')).toBeTruthy();
  });
});

it('switches tabs with the keyboard', async () => {
  render(SessionDialog, props);
  const modelTab = await screen.findByRole('tab', { name: 'Model' });
  modelTab.focus();
  await fireEvent.keyDown(modelTab, { key: 'ArrowRight' });
  expect(screen.getByRole('tab', { name: 'Context' })).toHaveProperty('ariaSelected', 'true');
  expect(screen.getByRole('tabpanel')).toHaveProperty('id', 'session-context-panel');
});
