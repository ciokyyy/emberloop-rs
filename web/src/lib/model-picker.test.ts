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

    const select = getByLabelText('Model') as HTMLInputElement;
    expect(select.value).toBe('Gemini 3.8 Flash');
    await fireEvent.click(getByRole('button', { name: 'Show model options' }));
    expect(getByRole('option', { name: /Gemini 3.8 Flash · 1M/ })).toBeTruthy();
    expect(getByRole('option', { name: /Gemini 2.5 Pro · 1M · stale/ })).toBeTruthy();

    await fireEvent.pointerUp(getByRole('option', { name: /Gemini 2.5 Pro/ }));
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

    const select = getByLabelText('Model') as HTMLInputElement;
    expect(select.value).toBe('Gemini 3.8 Flash');
    await fireEvent.click(getByRole('button', { name: 'Show model options' }));
    await fireEvent.pointerUp(getByRole('option', { name: /Gemini 2.5 Pro/ }));
    await fireEvent.click(getByRole('button', { name: 'Create' }));

    expect(oncreate).toHaveBeenCalledWith(
      expect.objectContaining({ projectId: 'proj_1', model: 'gemini-2.5-pro' })
    );
  });
});

it('filters models by label/provider without dropping the current selection', async () => {
  const view = render(ModelPicker, { models, value: 'gemini-3.8-flash', searchable: true });
  await fireEvent.input(view.getByRole('combobox', { name: 'Model' }), { target: { value: 'missing' } });
  expect(view.queryByRole('option', { name: /Gemini 2.5 Pro/ })).toBeNull();
  expect(view.getByRole('combobox')).toHaveProperty('value', 'missing');
  await fireEvent.input(view.getByRole('combobox', { name: 'Model' }), { target: { value: 'agy' } });
  expect(view.getByRole('option', { name: /Gemini 2.5 Pro/ })).toBeTruthy();
});

it('searches model names and groups matches by harness and provider', async () => {
  const { getByRole, queryByRole, getByText } = render(ModelPicker, { models, searchable: true });
  const input = getByRole('combobox', { name: 'Model' });
  await fireEvent.input(input, { target: { value: 'flash' } });
  expect(getByRole('option', { name: /Gemini 3.8 Flash/ })).toBeTruthy();
  expect(queryByRole('option', { name: /Gemini 2.5 Pro/ })).toBeNull();
  expect(getByText('Antigravity · agy')).toBeTruthy();
});

it('keeps the selected model when its option is selected again', async () => {
  const onchange = vi.fn();
  const view = render(ModelPicker, { models, value: 'gemini-3.8-flash', onchange });
  await fireEvent.click(view.getByRole('button', { name: 'Show model options' }));
  await fireEvent.pointerUp(view.getByRole('option', { name: /Gemini 3.8 Flash/ }));
  expect(view.getByRole('combobox')).toHaveProperty('value', 'Gemini 3.8 Flash');
  expect(onchange).not.toHaveBeenCalledWith('');
});

it('virtualizes long model lists and gives each option a provider logo', async () => {
  const many: Model[] = Array.from({ length: 300 }, (_, i) => ({ ...models[0], id: `m-${i}`, label: `Model ${i}` }));
  const view = render(ModelPicker, { models: many, value: 'm-0' });
  await fireEvent.click(view.getByRole('button', { name: 'Show model options' }));
  const rendered = view.getAllByRole('option');
  expect(rendered.length).toBeGreaterThan(0);
  expect(rendered.length).toBeLessThan(60);
  expect(rendered[0].querySelector('svg')).toBeTruthy();
});
