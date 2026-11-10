import type { Model } from './api/types';

/** The harness a model runs on, as a picker's group heading names it. */
function harness(model: Model) {
  return model.source === 'pi' ? 'pi-rs' : model.source === 'claude' ? 'Claude' : 'Antigravity';
}

/** Models whose id, label, harness or provider match `search`, grouped by harness and provider. */
export function modelGroups(models: Model[], search = ''): [string, Model[]][] {
  const needle = search.trim().toLowerCase();
  const grouped = new Map<string, Model[]>();
  for (const model of models) {
    const group = `${harness(model)} · ${model.provider}`;
    if (needle && !`${model.id} ${model.label} ${group}`.toLowerCase().includes(needle)) continue;
    grouped.set(group, [...(grouped.get(group) ?? []), model]);
  }
  return [...grouped].sort(([a], [b]) => a.localeCompare(b));
}

/** A context window in the short form a picker shows: `1M`, `200k`, or `window unknown`. */
export function windowLabel(model: Model) {
  if (!model.context_window) return 'window unknown';
  return model.context_window >= 1_000_000
    ? `${Math.round(model.context_window / 1_000_000)}M`
    : `${Math.round(model.context_window / 1000)}k`;
}
