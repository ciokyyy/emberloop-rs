import { fireEvent, render, screen } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import ContextDetails from './ContextDetails.svelte';

it('opens the Session cache page from the context details', async () => {
  const oncache = vi.fn();
  render(ContextDetails, { used: 1000, size: 200000, oncache });
  await fireEvent.click(screen.getByRole('button', { name: 'Prompt cache' }));
  expect(oncache).toHaveBeenCalledOnce();
});
