import { fireEvent, render, screen } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import Sidebar from './Sidebar.svelte';

const base = {
  models: [],
  statuses: {},
  onselect: vi.fn(),
  onnew: vi.fn(),
  onnewproject: vi.fn(),
  onselectproject: vi.fn(),
  onrename: vi.fn(),
  ondelete: vi.fn(),
  onsettings: vi.fn(),
  onproviders: vi.fn(),
  onusage: vi.fn()
};

it('colors by harness, not the Claude model name served by Antigravity', () => {
  render(Sidebar, {
    ...base,
    projects: [{ id: 'p', name: 'P', root: '/p' }],
    projectId: 'p',
    sessions: [{ id: 's', projectId: 'p', title: 'Test', model: 'claude-sonnet', createdAt: 0, updatedAt: 0 }],
    selectedId: 's'
  });
  expect(screen.getByTitle('Antigravity · Idle').style.background).toBe('rgb(91, 154, 198)');
});

it('gives pi-rs its own color, away from Claude clay', () => {
  render(Sidebar, {
    ...base,
    projects: [{ id: 'p', name: 'P', root: '/p' }],
    projectId: 'p',
    sessions: [{ id: 's', projectId: 'p', title: 'Test', model: 'openai/gpt-5', createdAt: 0, updatedAt: 0 }],
    selectedId: 's'
  });
  expect(screen.getByTitle('pi-rs · Idle').style.background).toBe('rgb(139, 124, 246)');
});

it('colors the draft row by the model picked before the Session exists', () => {
  render(Sidebar, {
    ...base,
    projects: [{ id: 'p', name: 'P', root: '/p' }],
    projectId: 'p',
    draftModel: 'openai/gpt-5',
    sessions: [], selectedId: null
  });
  expect(screen.getByTitle('pi-rs · Draft').style.background).toBe('rgb(139, 124, 246)');
});

it('names the Project in the header select instead of a brand heading', () => {
  render(Sidebar, {
    ...base,
    projects: [
      { id: 'p', name: 'Pi', root: '/p' },
      { id: 'q', name: 'Other', root: '/q' }
    ],
    projectId: 'p',
    sessions: [], selectedId: null
  });
  expect(screen.queryByText('Emberloop')).toBeNull();
  expect(screen.getByRole('combobox').textContent).toContain('Pi');
});

it('groups sessions by project, omitting projects with no sessions', () => {
  const now = Date.now();
  render(Sidebar, {
    ...base,
    projects: [
      { id: 'p', name: 'Pi Project', root: '/p' },
      { id: 'q', name: 'Other Project', root: '/q' },
      { id: 'empty', name: 'Empty Project', root: '/empty' }
    ],
    projectId: 'p',
    sessions: [
      { id: 'a', projectId: 'p', title: 'In Pi', model: 'x', createdAt: 0, updatedAt: now - 1000 },
      { id: 'b', projectId: 'q', title: 'In Other', model: 'x', createdAt: 0, updatedAt: now - 2000 }
    ],
    selectedId: null
  });
  const headers = screen.getAllByTestId('project-header').map((el) => el.textContent?.trim());
  expect(headers).toContain('Pi Project');
  expect(headers).toContain('Other Project');
  expect(headers).not.toContain('Empty Project');
  expect(screen.getByText('In Pi')).toBeTruthy();
  expect(screen.getByText('In Other')).toBeTruthy();
});

it('bands: working first, then the last 10 minutes, then the current Project', () => {
  const now = Date.now();
  render(Sidebar, {
    ...base,
    projects: [
      { id: 'p', name: 'Pi', root: '/p' },
      { id: 'q', name: 'Other', root: '/q' }
    ],
    projectId: 'p',
    statuses: { running: 'working' },
    sessions: [
      { id: 'old-other', projectId: 'q', title: 'Old Other', model: 'x', createdAt: 0, updatedAt: 1 },
      { id: 'fresh-other', projectId: 'q', title: 'Fresh Other', model: 'x', createdAt: 0, updatedAt: now - 1000 },
      { id: 'old-pi', projectId: 'p', title: 'Old Pi', model: 'x', createdAt: 0, updatedAt: 2 },
      { id: 'running', projectId: 'q', title: 'Running Other', model: 'x', createdAt: 0, updatedAt: now - 30 * 60 * 1000 }
    ],
    selectedId: null
  });
  const text = document.body.textContent ?? '';
  expect(screen.getAllByTestId('section-header').map((el) => el.textContent?.trim())).toEqual(['Active', 'Recent']);
  expect(text.indexOf('Running Other')).toBeLessThan(text.indexOf('Fresh Other'));
  expect(text.indexOf('Fresh Other')).toBeLessThan(text.indexOf('Old Pi'));
  expect(screen.queryByText('Old Other')).toBeNull();
});

it('renames on double-click, commits the title on Enter', async () => {
  const onrename = vi.fn();
  render(Sidebar, {
    ...base,
    onrename,
    projects: [{ id: 'p', name: 'P', root: '/p' }],
    projectId: 'p',
    sessions: [{ id: 's', projectId: 'p', title: 'Old', model: 'x', createdAt: 0, updatedAt: 0 }],
    selectedId: null
  });
  await fireEvent.click(screen.getByRole('button', { name: /Old/ }), { detail: 2 });
  const input = screen.getByLabelText('Session title');
  await fireEvent.input(input, { target: { value: 'New' } });
  await fireEvent.keyDown(input, { key: 'Enter' });
  expect(onrename).toHaveBeenCalledWith(expect.objectContaining({ id: 's' }), 'New');
});

it('asks before deleting: Delete key opens the dialog, confirm deletes', async () => {
  const ondelete = vi.fn();
  render(Sidebar, {
    ...base,
    ondelete,
    projects: [{ id: 'p', name: 'P', root: '/p' }],
    projectId: 'p',
    sessions: [{ id: 's', projectId: 'p', title: 'Gone', model: 'x', createdAt: 0, updatedAt: 0 }],
    selectedId: null
  });
  await fireEvent.keyDown(screen.getByRole('button', { name: /Gone/ }), { key: 'Delete' });
  expect(ondelete).not.toHaveBeenCalled();
  await fireEvent.click(await screen.findByRole('button', { name: 'Delete' }));
  expect(ondelete).toHaveBeenCalledWith(expect.objectContaining({ id: 's' }));
});

it('shows active working sessions across projects at the top with a working dot', () => {
  render(Sidebar, {
    ...base,
    projects: [{ id: 'p', name: 'Pi', root: '/p' }, { id: 'q', name: 'Other', root: '/q' }],
    projectId: 'p',
    statuses: { b: 'working' },
    sessions: [
      { id: 'a', projectId: 'p', title: 'In Pi', model: 'x', createdAt: 0, updatedAt: 2 },
      { id: 'b', projectId: 'q', title: 'Working In Other', model: 'x', createdAt: 0, updatedAt: 1 }
    ],
    selectedId: null
  });
  expect(screen.getByText('Working In Other')).toBeTruthy();
  expect(screen.getByLabelText('Working')).toBeTruthy();
  expect(screen.getByText('In Pi')).toBeTruthy();
});

it("keeps reviewer children after their parent and opens their own Session", async () => {
  const session = { projectId: "p", model: "gemini", createdAt: 1, updatedAt: 1 };
  const onselect = vi.fn();
  const view = render(Sidebar, { ...base, onselect, projects: [{ id: "p", name: "P", root: "/p" }], projectId: "p", selectedId: "main",
    sessions: [{ ...session, id: "child", title: "Spec reviewer", parentId: "main", reviewerRole: "spec", updatedAt: 9 }, { ...session, id: "main", title: "Main" }] });
  expect(view.container.textContent!.indexOf("Main")).toBeLessThan(view.container.textContent!.indexOf("Spec reviewer"));
  await fireEvent.click(view.getByText("Spec reviewer"));
  expect(onselect).toHaveBeenCalledWith("child");
});
