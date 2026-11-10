import { describe, expect, it } from 'vitest';
import type { Model } from '$lib/api/types';
import { modelGroups, windowLabel } from '$lib/models';

const model = (id: string, over: Partial<Model> = {}): Model => ({
  id, label: id, provider: 'agy', kind: 'chat', efforts: [], image_input: true,
  context_window: 200000, source: 'agy', stale: false, updated_at: 0, ...over
});

describe('modelGroups', () => {
  it('groups by harness and provider, sorted by heading', () => {
    const groups = modelGroups([model('b', { source: 'pi' }), model('a')]);
    expect(groups.map(([heading]) => heading)).toEqual(['Antigravity · agy', 'pi-rs · agy']);
  });

  it('keeps only models whose id, label, harness or provider match the search', () => {
    const models = [model('flash', { label: 'Gemini Flash' }), model('pro', { label: 'Gemini Pro', provider: 'openai' })];
    expect(modelGroups(models, 'flash').flatMap(([, items]) => items.map((item) => item.id))).toEqual(['flash']);
    expect(modelGroups(models, 'openai').flatMap(([, items]) => items.map((item) => item.id))).toEqual(['pro']);
    expect(modelGroups(models, '  ').flatMap(([, items]) => items)).toHaveLength(2);
  });
});

describe('windowLabel', () => {
  it('shortens a window and names a missing one', () => {
    expect(windowLabel(model('a', { context_window: 1_048_576 }))).toBe('1M');
    expect(windowLabel(model('a', { context_window: 200_000 }))).toBe('200k');
    expect(windowLabel(model('a', { context_window: 0 }))).toBe('window unknown');
  });
});
