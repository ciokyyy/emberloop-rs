import { fireEvent, render, screen } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import { api } from '$lib/api';
import ProjectDrawer from './ProjectDrawer.svelte';

vi.mock('$lib/api', () => ({ api: { updateProject: vi.fn() } }));

const project = { id: 'p1', name: 'emberloop-rs', root: '/project', tools: [] };

const toolProject = {
  ...project,
  tools: [
    { name: 'photocraft', url: 'https://box.ts.net/mcp', enabled: true },
    { name: 'docs', url: '', enabled: false }
  ]
};

it('slides the rail open and dismisses it by scrim, Close or Escape', async () => {
  const onclose = vi.fn();
  const { container } = render(ProjectDrawer, {
    project,
    open: true,
    onclose,
    onupdate: vi.fn(),
    onopendesign: vi.fn()
  });
  expect(container.querySelector('.drawer-rail.open')).toBeTruthy();

  await fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  await fireEvent.keyDown(window, { key: 'Escape' });
  await fireEvent.click(screen.getByRole('button', { name: 'Close project tools' }));
  expect(onclose).toHaveBeenCalledTimes(3);
});

it('keeps the rail mounted but hidden while closed', () => {
  const { container } = render(ProjectDrawer, {
    project,
    open: false,
    onclose: vi.fn(),
    onupdate: vi.fn(),
    onopendesign: vi.fn()
  });
  const rail = container.querySelector('.drawer-rail')!;
  expect(rail.classList.contains('open')).toBe(false);
  expect(rail.getAttribute('aria-hidden')).toBe('true');
  expect(container.querySelector('.drawer-scrim')).toBeNull();
});

it('lists Project tools, toggles them through PATCH and opens a link only when set', async () => {
  const onupdate = vi.fn();
  vi.mocked(api.updateProject).mockResolvedValue({
    ...toolProject,
    tools: [
      { name: 'photocraft', url: 'https://box.ts.net/mcp', enabled: false },
      { name: 'docs', url: '', enabled: false }
    ]
  });
  render(ProjectDrawer, {
    project: toolProject,
    open: true,
    onclose: vi.fn(),
    onupdate,
    onopendesign: vi.fn()
  });

  expect(screen.getByText('photocraft')).toBeTruthy();
  expect(screen.getByText('docs')).toBeTruthy();
  expect(screen.getByText('Tool changes apply to new Sessions.')).toBeTruthy();
  const link = screen.getByRole('link', { name: 'Open photocraft' }) as HTMLAnchorElement;
  expect(link.href).toBe('https://box.ts.net/mcp');
  expect(link.target).toBe('_blank');
  expect(screen.queryByRole('link', { name: 'Open docs' })).toBeNull();

  await fireEvent.click(screen.getByRole('switch', { name: 'photocraft enabled' }));
  const toggled = [
    { name: 'photocraft', url: 'https://box.ts.net/mcp', enabled: false },
    { name: 'docs', url: '', enabled: false }
  ];
  await vi.waitFor(() =>
    expect(api.updateProject).toHaveBeenCalledWith('p1', { tools: toggled })
  );
  await vi.waitFor(() => expect(onupdate).toHaveBeenCalledWith(expect.objectContaining({ tools: toggled })));
});
