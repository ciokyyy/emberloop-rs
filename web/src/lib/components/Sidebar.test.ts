import { render, screen } from '@testing-library/svelte';
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
    sessions: [{ id: 's', projectId: 'p', title: 'Test', model: 'claude-sonnet', ponytailLevel: 'full', createdAt: 0, updatedAt: 0 }],
    selectedId: 's'
  });
  expect(screen.getByTitle('Antigravity · Idle').style.background).toBe('rgb(91, 154, 198)');
});

it('gives pi-rs its own color, away from Claude clay', () => {
  render(Sidebar, {
    ...base,
    projects: [{ id: 'p', name: 'P', root: '/p' }],
    projectId: 'p',
    sessions: [{ id: 's', projectId: 'p', title: 'Test', model: 'openai/gpt-5', ponytailLevel: 'full', createdAt: 0, updatedAt: 0 }],
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

it('lists only the selected Project’s Sessions, with no Project header or count', () => {
  render(Sidebar, {
    ...base,
    projects: [{ id: 'p', name: 'Pi', root: '/p' }, { id: 'q', name: 'Other', root: '/q' }],
    projectId: 'p',
    sessions: [
      { id: 'a', projectId: 'p', title: 'In Pi', model: 'x', ponytailLevel: 'full', createdAt: 0, updatedAt: 2 },
      { id: 'b', projectId: 'q', title: 'In Other', model: 'x', ponytailLevel: 'full', createdAt: 0, updatedAt: 1 }
    ],
    selectedId: null
  });
  expect(screen.getByText('In Pi')).toBeTruthy();
  expect(screen.queryByText('In Other')).toBeNull();
  expect(screen.queryByText('1')).toBeNull();
});
