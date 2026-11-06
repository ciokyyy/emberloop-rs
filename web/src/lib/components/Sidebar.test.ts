import { render, screen } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import Sidebar from './Sidebar.svelte';

it('colors by harness, not the Claude model name served by Antigravity', () => {
  render(Sidebar, {
    projects: [{ id: 'p', name: 'P', root: '/p' }],
    sessions: [{ id: 's', projectId: 'p', title: 'Test', model: 'claude-sonnet', ponytailLevel: 'full', createdAt: 0, updatedAt: 0 }],
    statuses: {}, selectedId: 's', onselect: vi.fn(), onnew: vi.fn(), onnewproject: vi.fn(), onrename: vi.fn(), ondelete: vi.fn(), onsettings: vi.fn(), onproviders: vi.fn(), onusage: vi.fn()
  });
  expect(screen.getByTitle('Antigravity · Idle').style.background).toBe('rgb(91, 154, 198)');
});
