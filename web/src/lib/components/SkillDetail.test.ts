import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { api } from '$lib/api';
import SkillDetail from './SkillDetail.svelte';

afterEach(() => vi.restoreAllMocks());

it('diffs the newest backup against the current SKILL.md', async () => {
  vi.spyOn(api, 'skillDetail').mockResolvedValue({
    name: 'web-ui', description: '', project: true, origin: 'agent', path: '/p/.agents/skills/web-ui/SKILL.md',
    text: 'name: web-ui\nrun bun\n', files: [{ path: 'references/tokens.md', text: null }]
  });
  vi.spyOn(api, 'skillHistory').mockResolvedValue({
    backups: [{ at: 1739524320, file: 'SKILL.md', text: 'name: web-ui\nrun bun run check\n' }],
    usage: []
  });
  const onback = vi.fn();
  render(SkillDetail, { name: 'web-ui', onback });
  expect(await screen.findByText('- run bun run check')).toBeTruthy();
  expect(screen.getByText('+ run bun')).toBeTruthy();
  expect(screen.getByText('Files (1)')).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'All skills' }));
  expect(onback).toHaveBeenCalled();
});
