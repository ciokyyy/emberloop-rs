import { fireEvent, render } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import ModelPicker from '$lib/components/ModelPicker.svelte';
import NewSessionDialog from '$lib/components/NewSessionDialog.svelte';
import type { Model } from '$lib/api/types';

const models: Model[] = [
  {
    id: 'gemini-3.8-flash',
    provider: 'agy',
    label: 'Gemini 3.8 Flash',
    kind: 'chat',
    efforts: [],
    image_input: true,
    context_window: 1_048_576,
    source: 'agy',
    stale: false,
    updated_at: 0
  },
  {
    id: 'gemini-2.5-pro',
    provider: 'agy',
    label: 'Gemini 2.5 Pro',
    kind: 'chat',
    efforts: ['high'],
    image_input: true,
    context_window: 1_048_576,
    source: 'agy',
    stale: true,
    updated_at: 0
  }
];

describe('ModelPicker', () => {
  it('lists models with their context window and reports a selection', async () => {
    const onchange = vi.fn();
    const { getByLabelText, getByRole } = render(ModelPicker, {
      models,
      value: 'gemini-3.8-flash',
      onchange
    });

    const select = getByLabelText('Model') as HTMLSelectElement;
    expect(select.value).toBe('gemini-3.8-flash');
    expect(getByRole('option', { name: /Gemini 3.8 Flash · 1M/ })).toBeTruthy();
    expect(getByRole('option', { name: /Gemini 2.5 Pro · 1M · stale/ })).toBeTruthy();

    await fireEvent.change(select, { target: { value: 'gemini-2.5-pro' } });
    expect(onchange).toHaveBeenCalledWith('gemini-2.5-pro');
  });
});

describe('NewSessionDialog model picker', () => {
  it('carries the selected model into the create request', async () => {
    const oncreate = vi.fn();
    const { getByLabelText, getByRole } = render(NewSessionDialog, {
      open: true,
      projects: [{ id: 'proj_1', name: 'Demo', root: '/tmp/demo' }],
      models,
      oncreate
    });

    const select = getByLabelText('Model') as HTMLSelectElement;
    expect(select.value).toBe('gemini-3.8-flash');
    await fireEvent.change(select, { target: { value: 'gemini-2.5-pro' } });
    await fireEvent.click(getByRole('button', { name: 'Create' }));

    expect(oncreate).toHaveBeenCalledWith(
      expect.objectContaining({ projectId: 'proj_1', model: 'gemini-2.5-pro' })
    );
  });
});
