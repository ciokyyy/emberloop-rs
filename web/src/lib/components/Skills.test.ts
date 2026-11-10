import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { api } from '$lib/api';
import Skills from './Skills.svelte';

afterEach(() => vi.restoreAllMocks());

it('groups by scope and opens the Skill the row names', async () => {
  vi.spyOn(api, 'listSkills').mockResolvedValue([
    { name: 'web-ui', description: 'SvelteKit work', project: true, origin: 'agent', path: '/p/.agents/skills/web-ui/SKILL.md' },
    { name: 'research', description: 'Sources', project: false, origin: 'human', path: '/g/research/SKILL.md' }
  ]);
  const onopen = vi.fn();
  render(Skills, { projectId: 'p', onopen });
  expect(await screen.findByText('This project (1)')).toBeTruthy();
  expect(screen.getByText('Global (1)')).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: /research/ }));
  expect(onopen).toHaveBeenCalledWith('research');
});
