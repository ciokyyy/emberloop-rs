import { fireEvent, render, within } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import type { Model, Settings as SettingsData } from '$lib/api/types';
import Settings from './Settings.svelte';

vi.mock('$lib/api', () => ({ api: { listToolKeys: async () => [], listPrompts: async () => [] } }));

const saved: SettingsData = {
  title_generation: true, title_model: 'model', default_model: 'model',
  spec_reviewer: { model: 'model', reasoning_effort: null },
  standards_reviewer: { model: 'model', reasoning_effort: null },
  proxy_compression: true, ponytail_default_level: 'full', todo_reminders: true,
  todo_reminders_max: 3, todo_auto_expand: true, todo_hide_when_done: true,
  max_context_limit: 75, min_pressure_benefit_tokens: 0, emergency_threshold_percent: 95,
  preserve_recent_messages: 3, protected_tools: [], inject_tool: true, inject_nudge: true,
  tier2_trigger: 1000, tier3_trigger: 2000, skills_review_interval: 5
};
const models: Model[] = [{ id: 'model', provider: 'pi', label: 'Model', kind: 'chat', efforts: ['low', 'high'],
  image_input: false, context_window: 100000, source: 'pi', stale: false, updated_at: 0 }];

it('saves independent reviewer reasoning defaults without mutating saved settings', async () => {
  const onsave = vi.fn();
  const view = render(Settings, { settings: saved, models, onsave });
  const spec = within(view.getByRole('group', { name: 'Spec reviewer reasoning effort', hidden: true }));
  const standards = within(view.getByRole('group', { name: 'Standards reviewer reasoning effort', hidden: true }));
  await fireEvent.click(spec.getByRole('button', { name: 'low', hidden: true }));
  await fireEvent.click(standards.getByRole('button', { name: 'high', hidden: true }));
  await fireEvent.click(view.getByRole('button', { name: 'Save' }));
  expect(onsave).toHaveBeenCalledWith(expect.objectContaining({
    spec_reviewer: { model: 'model', reasoning_effort: 'low' },
    standards_reviewer: { model: 'model', reasoning_effort: 'high' }
  }));
  expect(saved.spec_reviewer.reasoning_effort).toBeNull();
});
