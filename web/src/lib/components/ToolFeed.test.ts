import { fireEvent, render } from '@testing-library/svelte';
import { expect, it } from 'vitest';
import ToolFeed from './ToolFeed.svelte';
import type { ToolRun } from '$lib/toolRuns';

const run = (
  name: string,
  input: Record<string, unknown>,
  output = '',
  extra: Partial<ToolRun> = {},
): ToolRun => ({
  name,
  input: JSON.stringify(input),
  output,
  status: 'completed',
  elapsedMs: 900,
  ...extra,
});

const iconOf = (row: HTMLElement | null) =>
  row?.querySelector<HTMLElement>('[data-file-icon]')?.dataset.fileIcon ?? null;

it('icons file rows by extension when expanded, keeps rows closed by default, and leaves Ran rows bare', async () => {
  const view = render(ToolFeed, {
    runs: [
      run('read', { path: 'src/a.ts' }, 'const a = 1;'),
      run('edit', { path: 'Cargo.toml' }, '--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1 +1 @@\n-a\n+b'),
      run('bash', { command: 'ls' }, 'ok'),
    ],
  });

  // Group starts closed; expanding reveals read children with file icons.
  expect(view.container.querySelector('.group-child')).toBeNull();
  await fireEvent.click(view.container.querySelector('.group-header')!);
  const read = iconOf(view.container.querySelector<HTMLElement>('.group-child'));
  const edit = iconOf(view.container.querySelector<HTMLElement>('.edit-row'));
  expect(read).toBeTruthy();
  expect(edit).toBeTruthy();
  expect(read).not.toBe(edit);
  expect(iconOf(view.container.querySelector<HTMLElement>('.ran-row'))).toBeNull();

  // Last row (bash) stays closed by default.
  expect(view.container.querySelector('.ran-row .expanded-box')).toBeNull();
});

it('keeps the last row closed (including running) while failed rows open by default', () => {
  const view = render(ToolFeed, {
    runs: [
      run('bash', { command: 'false' }, 'boom\n[exit status 1]', { status: 'failed' }),
      run('execute', { code: '# Check build\nimport tools\ntools.read(path="a.rs")' }, '', {
        status: 'running',
      }),
    ],
  });

  expect(view.container.querySelector('[data-testid="failed-row"] .expanded-box')).toBeTruthy();
  expect(view.container.querySelector('[data-testid="toolfeed-execute-row"] .expanded-box')).toBeNull();
});

it('renders submit as its own row, agy intent on rows, and execute children when expanded', async () => {
  const view = render(ToolFeed, {
    runs: [
      run('el_bash', { command: './scripts/tail.sh' }, 'started', {
        display: { title: 'Restarting wrangler tail in background' },
      }),
      run('execute', { code: 'import tools\ntools.read(path="src/lib.rs")\ntools.bash(command="cargo check")' }, 'ok', {
        children: [
          { name: 'read', input: '{"path":"src/lib.rs"}', output: '', status: 'completed', elapsedMs: 12, target: 'src/lib.rs' },
          { name: 'bash', input: '{"command":"cargo check"}', output: '', status: 'completed', elapsedMs: 240, target: 'cargo check' },
        ],
      }),
      run('submit', { status: 'done', summary: 'All tests green' }, 'submitted'),
    ],
  });

  expect(view.getByText('Restarting wrangler tail in background')).toBeTruthy();
  const submitRow = view.container.querySelector<HTMLElement>('.submit-row');
  expect(submitRow?.textContent).toContain('Submitted report');
  expect(submitRow?.textContent).not.toContain('Ran');

  const execRow = view.container.querySelector<HTMLElement>('[data-testid="toolfeed-execute-row"]');
  expect(execRow).toBeTruthy();
  expect(execRow!.querySelector('.expanded-box')).toBeNull();
  await fireEvent.click(execRow!.querySelector('button')!);
  expect(execRow!.querySelector('.child-row')?.textContent).toContain('src/lib.rs');
});

it('handles empty finished thinking, live empty thinking pulse, and first-sentence thinking toggle', async () => {
  const view = render(ToolFeed, {
    runs: [
      { name: 'thinking', input: '', output: '   ', status: 'completed', elapsedMs: 0, thinking: true, live: false },
      { name: 'thinking', input: '', output: '', status: 'running', elapsedMs: 0, thinking: true, live: true },
      {
        name: 'thinking',
        input: '',
        output: 'Check the reducer first. Then verify the component.',
        status: 'completed',
        elapsedMs: 3200,
        thinking: true,
      },
    ],
  });

  expect(view.getByTestId('thinking-pulse')).toBeTruthy();
  const blocks = view.container.querySelectorAll('.thinking-block');
  expect(blocks.length).toBe(1);
  expect(blocks[0].textContent).toContain('Check the reducer first.');
  expect(blocks[0].textContent).not.toContain('Then verify the component.');
  await fireEvent.click(blocks[0].querySelector('button')!);
  expect(blocks[0].textContent).toContain('Then verify the component.');
});
