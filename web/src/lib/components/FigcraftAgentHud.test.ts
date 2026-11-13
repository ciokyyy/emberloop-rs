import { fireEvent, render, screen } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import FigcraftAgentHud from './FigcraftAgentHud.svelte';

it('keeps the agent prompt collapsed behind a toggleable pill', async () => {
  render(FigcraftAgentHud, {
    project: { id: 'p1', name: 'Design', root: '/project' },
    sessions: [{ id: 's1', projectId: 'p1', title: 'Designer', model: 'test', createdAt: 1, updatedAt: 1 }],
    currentSessionId: 's1',
    onsendprompt: vi.fn()
  });
  expect(screen.queryByRole('textbox')).toBeNull();
  await fireEvent.click(screen.getByRole('button', { name: 'Open agent prompt' }));
  expect(screen.getByRole('textbox')).toBeTruthy();
  // The board's session is the agent: no switcher inside the HUD.
  expect(screen.queryByRole('combobox')).toBeNull();
  await fireEvent.click(screen.getByRole('button', { name: 'Collapse agent prompt' }));
  expect(screen.queryByRole('textbox')).toBeNull();
});

it('keeps failed prompts and shows the error without leaving the editor', async () => {
  render(FigcraftAgentHud, {
    project: { id: 'p1', name: 'Design', root: '/project' },
    sessions: [{ id: 's1', projectId: 'p1', title: 'Designer', model: 'test', createdAt: 1, updatedAt: 1 }],
    currentSessionId: 's1',
    onsendprompt: vi.fn().mockRejectedValue(new Error('Agent unavailable'))
  });
  await fireEvent.click(screen.getByRole('button', { name: 'Open agent prompt' }));
  await fireEvent.input(screen.getByRole('textbox'), { target: { value: 'Add a button' } });
  await fireEvent.submit(screen.getByRole('textbox').closest('form')!);
  expect((await screen.findByRole('alert')).textContent).toContain('Agent unavailable');
  expect((screen.getByRole('textbox') as HTMLInputElement).value).toBe('Add a button');
});

it('drags the pill and clamps it back inside a resized viewport', async () => {
  // jsdom has no PointerEvent, so fireEvent would drop clientX/clientY without this stand-in.
  Object.defineProperty(window, 'PointerEvent', { value: class extends MouseEvent {}, configurable: true });
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockReturnValue({
    left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0, x: 0, y: 0, toJSON: () => ({})
  } as DOMRect);
  Object.defineProperty(window, 'innerWidth', { value: 1024, configurable: true });
  render(FigcraftAgentHud, {
    project: { id: 'p1', name: 'Design', root: '/project' },
    sessions: [{ id: 's1', projectId: 'p1', title: 'Designer', model: 'test', createdAt: 1, updatedAt: 1 }],
    currentSessionId: 's1',
    onsendprompt: vi.fn()
  });
  const pill = screen.getByRole('button', { name: 'Open agent prompt' });
  await fireEvent.pointerDown(pill, { clientX: 500, clientY: 500 });
  await fireEvent.pointerMove(pill, { clientX: 5000, clientY: 5000 });
  await fireEvent.pointerUp(pill);
  const root = pill.parentElement!.parentElement as HTMLElement;
  expect(root.style.transform).toContain('translate(1016px, 760px)');

  // A narrower window must pull the dragged pill back so it stays reachable.
  Object.defineProperty(window, 'innerWidth', { value: 400, configurable: true });
  await fireEvent(window, new Event('resize'));
  expect(root.style.transform).toContain('translate(392px, 760px)');
});

it('leaves the full-width phone sheet flush with the viewport edges', async () => {
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockReturnValue({
    left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0, x: 0, y: 0, toJSON: () => ({})
  } as DOMRect);
  Object.defineProperty(window, 'innerWidth', { value: 390, configurable: true });
  render(FigcraftAgentHud, {
    project: { id: 'p1', name: 'Design', root: '/project' },
    sessions: [{ id: 's1', projectId: 'p1', title: 'Designer', model: 'test', createdAt: 1, updatedAt: 1 }],
    currentSessionId: 's1',
    onsendprompt: vi.fn()
  });
  const pill = screen.getByRole('button', { name: 'Open agent prompt' });
  await fireEvent.click(pill);
  // The sheet spans inset-x-0, so any clamp margin would shift it off the right edge.
  expect((pill.parentElement!.parentElement as HTMLElement).style.transform).toMatch(/^translate\(0px(?:, 0px)?\)$/);
});
