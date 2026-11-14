import { fireEvent, render, within } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import type { Model, Settings as SettingsData } from '$lib/api/types';
import Settings from './Settings.svelte';

vi.mock('$lib/api', () => ({ api: { listToolKeys: async () => [], listPrompts: async () => [] } }));

const saved: SettingsData = {
  title_generation: true, title_model: 'model', default_model: 'model',
  spec_reviewer: [{ model: 'model', reasoning_effort: null }],
  standards_reviewer: [{ model: 'model', reasoning_effort: null }],
  subagent_tiers: {
    smol: [{ model: 'model', reasoning_effort: null }],
    mid: [{ model: 'model', reasoning_effort: null }],
    top: [{ model: 'model', reasoning_effort: null }]
  },
  proxy_compression: true, ponytail_default_level: 'full', todo_reminders: true,
  todo_reminders_max: 3, todo_auto_expand: true, todo_hide_when_done: true,
  max_context_limit: 75, min_pressure_benefit_tokens: 0, emergency_threshold_percent: 95,
  preserve_recent_messages: 3, protected_tools: [], inject_tool: true, inject_nudge: true,
  tier2_trigger: 1000, tier3_trigger: 2000, skills_review_interval: 5
};
const models: Model[] = [
  { id: 'model', provider: 'pi', label: 'Model', kind: 'chat', efforts: ['low', 'high'],
    image_input: false, context_window: 100000, source: 'pi', stale: false, updated_at: 0 },
  { id: 'fast', provider: 'agy', label: 'Fast', kind: 'chat', efforts: ['low'],
    image_input: false, context_window: 100000, source: 'agy', stale: false, updated_at: 0 }
];
it('saves independent reviewer reasoning defaults without mutating saved settings', async () => {
  const onsave = vi.fn();
  const view = render(Settings, { settings: saved, models, onsave });
  const spec = within(view.getByRole('group', { name: 'Spec reviewer rung 1 reasoning effort', hidden: true }));
  const standards = within(view.getByRole('group', { name: 'Standards reviewer rung 1 reasoning effort', hidden: true }));
  await fireEvent.click(spec.getByRole('button', { name: 'low', hidden: true }));
  await fireEvent.click(standards.getByRole('button', { name: 'high', hidden: true }));
  await fireEvent.click(view.getByRole('button', { name: 'Save' }));
  expect(onsave).toHaveBeenCalledWith(expect.objectContaining({
    spec_reviewer: [{ model: 'model', reasoning_effort: 'low' }],
    standards_reviewer: [{ model: 'model', reasoning_effort: 'high' }]
  }));
  expect(saved.spec_reviewer[0].reasoning_effort).toBeNull();
});

it('saves intelligence tier presets and resets effort on model change without mutating saved settings', async () => {
  const onsave = vi.fn();
  const view = render(Settings, { settings: saved, models, onsave });
  const smol = within(view.getByRole('group', { name: 'Smol tier rung 1 reasoning effort', hidden: true }));
  const top = within(view.getByRole('group', { name: 'Top tier rung 1 reasoning effort', hidden: true }));
  await fireEvent.click(smol.getByRole('button', { name: 'low', hidden: true }));
  await fireEvent.click(top.getByRole('button', { name: 'high', hidden: true }));
  await fireEvent.click(view.getByRole('button', { name: 'Show smol tier rung 1 model options', hidden: true }));
  await fireEvent.pointerUp(view.getByRole('option', { name: /Fast/, hidden: true }));
  await fireEvent.click(view.getByRole('button', { name: 'Save' }));
  expect(onsave).toHaveBeenCalledWith(expect.objectContaining({
    subagent_tiers: {
      smol: [{ model: 'fast', reasoning_effort: null }],
      mid: [{ model: 'model', reasoning_effort: null }],
      top: [{ model: 'model', reasoning_effort: 'high' }]
    }
  }));
  expect(saved.subagent_tiers.smol).toEqual([{ model: 'model', reasoning_effort: null }]);
});

it('adds, reorders, and removes fallback ladder rungs without mutating saved settings', async () => {
  const onsave = vi.fn();
  const view = render(Settings, { settings: saved, models, onsave });
  const singleRemove = view.getByRole('button', { name: 'Remove Spec reviewer rung 1', hidden: true }) as HTMLButtonElement;
  expect(singleRemove.disabled).toBe(true);

  await fireEvent.click(view.getByRole('button', { name: 'Add Spec reviewer fallback', hidden: true }));
  await fireEvent.click(view.getByRole('button', { name: 'Show spec reviewer rung 2 model options', hidden: true }));
  await fireEvent.pointerUp(view.getByRole('option', { name: /Fast/, hidden: true }));
  const rung2Effort = within(view.getByRole('group', { name: 'Spec reviewer rung 2 reasoning effort', hidden: true }));
  await fireEvent.click(rung2Effort.getByRole('button', { name: 'low', hidden: true }));

  // Move rung 2 up so Fast becomes primary and Model becomes fallback.
  await fireEvent.click(view.getByRole('button', { name: 'Move Spec reviewer rung 2 up', hidden: true }));

  // Add two fallbacks on Mid tier then remove the middle one.
  await fireEvent.click(view.getByRole('button', { name: 'Add Mid tier fallback', hidden: true }));
  await fireEvent.click(view.getByRole('button', { name: 'Add Mid tier fallback', hidden: true }));
  await fireEvent.click(view.getByRole('button', { name: 'Show mid tier rung 3 model options', hidden: true }));
  await fireEvent.pointerUp(view.getByRole('option', { name: /Fast/, hidden: true }));
  await fireEvent.click(view.getByRole('button', { name: 'Remove Mid tier rung 2', hidden: true }));

  await fireEvent.click(view.getByRole('button', { name: 'Save' }));
  expect(onsave).toHaveBeenCalledWith(expect.objectContaining({
    spec_reviewer: [
      { model: 'fast', reasoning_effort: 'low' },
      { model: 'model', reasoning_effort: null }
    ],
    subagent_tiers: expect.objectContaining({
      mid: [
        { model: 'model', reasoning_effort: null },
        { model: 'fast', reasoning_effort: null }
      ]
    })
  }));
  expect(saved.spec_reviewer).toEqual([{ model: 'model', reasoning_effort: null }]);
  expect(saved.subagent_tiers.mid).toEqual([{ model: 'model', reasoning_effort: null }]);
});
