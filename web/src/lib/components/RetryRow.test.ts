import { render } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import RetryRow from './RetryRow.svelte';

const props = (retry: 'retrying' | 'finished', text: string) => ({ retry, text });

describe('harness retry status row', () => {
  it('is one muted status line with a failing dot, never a bubble', () => {
    const view = render(RetryRow, props('retrying', 'Retrying (attempt 1/3, waiting 2s)'));
    const row = view.getByTestId('retry-row');
    expect(row.getAttribute('data-state')).toBe('retrying');
    expect(row.textContent?.trim()).toBe('Retrying (attempt 1/3, waiting 2s)');
    expect(row.className).toContain('text-fg-3');
    expect(row.querySelector('span')?.className).toContain('bg-status-failed');
    expect(row.querySelector('.rounded-2xl')).toBeNull();
  });

  it('recolors the same dot when the retry finished', async () => {
    const view = render(RetryRow, props('retrying', 'Retrying (attempt 1/3, waiting 2s)'));
    await view.rerender(props('finished', 'Retry finished'));
    const row = view.getByTestId('retry-row');
    expect(row.getAttribute('data-state')).toBe('finished');
    expect(row.querySelector('span')?.className).toContain('bg-fg-4');
    expect(row.textContent?.trim()).toBe('Retry finished');
  });
});
