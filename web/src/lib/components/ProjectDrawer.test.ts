import { fireEvent, render, screen } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import ProjectDrawer from './ProjectDrawer.svelte';

const project = { id: 'p1', name: 'emberloop-rs', root: '/project' };

it('slides the rail open and dismisses it by scrim, Close or Escape', async () => {
  const onclose = vi.fn();
  const { container } = render(ProjectDrawer, { project, open: true, onclose, onopendesign: vi.fn() });
  expect(container.querySelector('.drawer-rail.open')).toBeTruthy();

  await fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  await fireEvent.keyDown(window, { key: 'Escape' });
  await fireEvent.click(screen.getByRole('button', { name: 'Close project tools' }));
  expect(onclose).toHaveBeenCalledTimes(3);
});

it('keeps the rail mounted but hidden while closed', () => {
  const { container } = render(ProjectDrawer, { project, open: false, onclose: vi.fn(), onopendesign: vi.fn() });
  const rail = container.querySelector('.drawer-rail')!;
  expect(rail.classList.contains('open')).toBe(false);
  expect(rail.getAttribute('aria-hidden')).toBe('true');
  expect(container.querySelector('.drawer-scrim')).toBeNull();
});
