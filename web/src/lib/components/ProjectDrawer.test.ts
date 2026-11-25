import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, expect, it, vi } from 'vitest';
import { api } from '$lib/api';
import ProjectDrawer from './ProjectDrawer.svelte';

vi.mock('$lib/api', () => ({ api: { updateProject: vi.fn(), listCraft: vi.fn() } }));

const figcraft = {
  name: 'figcraft',
  label: 'FigCraft',
  summary: 'FigCraft design tool.',
  url: 'https://getartcraft.com/apps/figcraft',
  installed: true
};
const photocraft = {
  name: 'photocraft',
  label: 'PhotoCraft',
  summary: 'PhotoCraft photo editor.',
  url: 'https://getartcraft.com/apps/photocraft',
  installed: false
};

beforeEach(() => {
  vi.mocked(api.listCraft).mockResolvedValue([figcraft]);
});

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

it('turns the built-in FigCraft server on for a Project and off again', async () => {
  const onupdate = vi.fn();
  vi.mocked(api.updateProject).mockResolvedValue({
    ...project,
    tools: [{ name: 'figcraft', url: '', enabled: true }]
  });
  const { unmount } = render(ProjectDrawer, {
    project,
    open: true,
    onclose: vi.fn(),
    onupdate,
    onopendesign: vi.fn()
  });
  const off = await screen.findByRole('switch', { name: 'FigCraft enabled' });
  expect(off.getAttribute('aria-checked')).toBe('false');

  await fireEvent.click(off);
  await vi.waitFor(() =>
    expect(api.updateProject).toHaveBeenCalledWith('p1', {
      tools: [{ name: 'figcraft', url: '', enabled: true }]
    })
  );
  await vi.waitFor(() => expect(onupdate).toHaveBeenCalled());
  unmount();

  const enabled = { ...project, tools: [{ name: 'figcraft', url: '', enabled: true }] };
  vi.mocked(api.updateProject).mockResolvedValue({
    ...enabled,
    tools: [{ name: 'figcraft', url: '', enabled: false }]
  });
  render(ProjectDrawer, {
    project: enabled,
    open: true,
    onclose: vi.fn(),
    onupdate: vi.fn(),
    onopendesign: vi.fn()
  });
  const on = await screen.findByRole('switch', { name: 'FigCraft enabled' });
  expect(on.getAttribute('aria-checked')).toBe('true');
  await fireEvent.click(on);
  await vi.waitFor(() =>
    expect(api.updateProject).toHaveBeenLastCalledWith('p1', {
      tools: [{ name: 'figcraft', url: '', enabled: false }]
    })
  );
});

it('lists every Craft app the daemon reports and switches one on through PATCH', async () => {
  vi.mocked(api.listCraft).mockResolvedValue([figcraft, photocraft]);
  const onupdate = vi.fn();
  vi.mocked(api.updateProject).mockResolvedValue({
    ...project,
    tools: [{ name: 'photocraft', url: '', enabled: true }]
  });
  render(ProjectDrawer, {
    project,
    open: true,
    onclose: vi.fn(),
    onupdate,
    onopendesign: vi.fn()
  });

  expect(await screen.findByText('PhotoCraft')).toBeTruthy();
  expect(screen.getByText('Built-in')).toBeTruthy();
  expect(screen.getByText('Craft apps')).toBeTruthy();
  expect(screen.getByText('not installed')).toBeTruthy();
  const page = screen.getByRole('link', { name: 'Open PhotoCraft' }) as HTMLAnchorElement;
  expect(page.href).toBe('https://getartcraft.com/apps/photocraft');
  expect(page.rel).toBe('noopener');
  const design = screen.getByRole('button', { name: 'Open FigCraft' });
  expect(design.getAttribute('title')).toBe('Open the design view');
  await fireEvent.click(screen.getByRole('switch', { name: 'PhotoCraft enabled' }));
  await vi.waitFor(() =>
    expect(api.updateProject).toHaveBeenCalledWith('p1', {
      tools: [{ name: 'photocraft', url: '', enabled: true }]
    })
  );
});

it('keeps the built-in FigCraft row when the Craft list fails', async () => {
  vi.mocked(api.listCraft).mockRejectedValue(new Error('offline'));
  render(ProjectDrawer, {
    project,
    open: true,
    onclose: vi.fn(),
    onupdate: vi.fn(),
    onopendesign: vi.fn()
  });

  expect(await screen.findByRole('switch', { name: 'FigCraft enabled' })).toBeTruthy();
  expect(screen.getByRole('alert').textContent).toBe('offline');
});
