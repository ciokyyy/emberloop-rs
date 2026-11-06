import { render } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import ContextRing from './ContextRing.svelte';
import ContextDetails from './ContextDetails.svelte';

describe('unknown context is not zero context', () => {
  it('shows unavailable usage until a provider count arrives', () => {
    const view = render(ContextRing, { size: 200000 });
    expect(view.getByTestId('context-ring').getAttribute('aria-label')).toMatch(/usage unavailable/i);
    expect(view.getByText('–')).toBeTruthy();
    expect(view.queryByText('0%')).toBeNull();
  });
  it('keeps an actual reported zero distinct from a small nonzero input', async () => {
    const view = render(ContextRing, { used: 0, size: 200000 });
    expect(view.getByText('0%')).toBeTruthy();
    await view.rerender({ used: 200 });
    expect(view.getByText('<1%')).toBeTruthy();
  });
  it('does not invent a zero provider total in the context dialog', () => {
    const view = render(ContextDetails, { size: 200000 });
    expect(view.getByText('Not reported yet')).toBeTruthy();
  });
});
