import { fireEvent, render, screen } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import SessionMenu from './SessionMenu.svelte';

const project = { id: 'p1', name: 'emberloop-rs', root: '/project' } as never;
const session = { id: 's1', projectId: 'p1', title: 'Cache work' } as never;
const link = `${window.location.origin}/p/p1/s/s1`;

function clipboard() {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
  return writeText;
}

it('opens the menu after a long press and copies the link or the reference block', async () => {
  const writeText = clipboard();
  const { container } = render(SessionMenu, { session, project });

  await fireEvent.pointerDown(container.querySelector('div.min-w-0')!, { button: 0, clientX: 0, clientY: 0 });
  await new Promise((resolve) => setTimeout(resolve, 520));

  await fireEvent.click(screen.getByText('Copy link'));
  expect(writeText).toHaveBeenCalledWith(link);

  await fireEvent.click(screen.getByText('Copy session reference'));
  expect(writeText).toHaveBeenLastCalledWith(
    `Session: Cache work\nProject: emberloop-rs (/project)\nURL: ${link}`
  );
});

it('opens the menu on a right click without a hold', async () => {
  clipboard();
  const { container } = render(SessionMenu, { session, project });
  await fireEvent.contextMenu(container.querySelector('div.min-w-0')!);
  expect(screen.getByText('Copy link')).toBeTruthy();
});
