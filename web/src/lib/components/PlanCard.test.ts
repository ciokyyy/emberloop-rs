import { render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { TodoItem } from '$lib/feed';
import PlanCard from './PlanCard.svelte';

const item = (id: string, content: string, status: TodoItem['status'] = 'pending'): TodoItem => ({
  id,
  content,
  status
});

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

const rows = (view: { queryAllByTestId: (id: string) => HTMLElement[] }) =>
  view.queryAllByTestId('plan-row');

describe('plan card', () => {
  it('renders nothing without items', () => {
    const view = render(PlanCard, { items: [] });
    expect(view.queryByTestId('plan-card')).toBeNull();
  });

  it('hides at once when every item is resolved and the fade is off', () => {
    const view = render(PlanCard, {
      items: [item('t1', 'one', 'completed')],
      hideWhenDone: false
    });
    expect(view.queryByTestId('plan-card')).toBeNull();
  });

  it('collapses to done/total, the active item and a progress bar', () => {
    const view = render(PlanCard, {
      items: [item('t1', 'read the spec', 'completed'), item('t2', 'write the parser', 'in_progress')],
      autoExpand: false
    });
    expect(view.getByTestId('plan-card').textContent).toContain('1/2');
    expect(view.getByTestId('plan-active').textContent).toContain('t2 write the parser');
    expect(view.getByTestId('plan-progress')).toBeTruthy();
    expect(rows(view)).toHaveLength(0);
  });

  it('toggles on click and on the p key', async () => {
    const view = render(PlanCard, {
      items: [item('t1', 'one')],
      autoExpand: false
    });
    await view.getByTestId('plan-toggle').click();
    expect(rows(view)).toHaveLength(1);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'p' }));
    await tick();
    expect(rows(view)).toHaveLength(0);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'p' }));
    await tick();
    expect(rows(view)).toHaveLength(1);
  });

  it('caps the expanded rows at eight and repeats the active item outside them', async () => {
    const items = Array.from({ length: 10 }, (_, index) => item(`t${index + 1}`, `item ${index + 1}`));
    items[9] = item('t10', 'item 10', 'in_progress');
    const view = render(PlanCard, { items, autoExpand: false });
    await view.getByTestId('plan-toggle').click();
    // Eight visible rows plus the repeated active one.
    expect(rows(view)).toHaveLength(9);
    expect(view.getByTestId('plan-more').textContent).toContain('+2 more');
    expect(rows(view)[0].textContent).toContain('item 10');
  });

  it('marks every status and strikes through the resolved ones', async () => {
    const view = render(PlanCard, {
      items: [
        item('t1', 'one'),
        item('t2', 'two', 'in_progress'),
        item('t3', 'three', 'completed'),
        item('t4', 'four', 'cancelled')
      ],
      autoExpand: false
    });
    await view.getByTestId('plan-toggle').click();
    const text = rows(view).map((row) => row.textContent);
    expect(text[0]).toContain('[ ]');
    expect(text[1]).toContain('[~]');
    expect(text[2]).toContain('[x]');
    expect(text[3]).toContain('[–]');
    expect(rows(view)[2].className).toContain('line-through');
    expect(rows(view)[3].className).toContain('line-through');
    expect(rows(view)[0].className).not.toContain('line-through');
  });

  it('opens for two seconds on a create and then collapses', async () => {
    const view = render(PlanCard, { items: [item('t1', 'one')] });
    expect(rows(view)).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(2000);
    await tick();
    expect(rows(view)).toHaveLength(0);
  });

  it('fades a few seconds after the last item resolves', async () => {
    const view = render(PlanCard, {
      items: [item('t1', 'one', 'completed')],
      hideWhenDone: true
    });
    expect(view.queryByTestId('plan-card')).not.toBeNull();
    await vi.advanceTimersByTimeAsync(4000);
    await tick();
    expect(view.queryByTestId('plan-card')).toBeNull();
  });
});
