import { render } from '@testing-library/svelte';
import { expect, it } from 'vitest';
import ToolBody from './ToolBody.svelte';

it('does not tint a markdown list or a + line as a diff', () => {
  const view = render(ToolBody, { text: '- item\n+ plus\nplain', path: '/a/notes.md' });
  expect(view.container.querySelector('.diff-del')).toBeNull();
  expect(view.container.querySelector('.diff-add')).toBeNull();
});

it('tints the rows of a real diff', () => {
  const view = render(ToolBody, { text: '-old\n+new', path: '/a/x.rs', language: 'diff' });
  expect(view.container.querySelector('.diff-del')).toBeTruthy();
  expect(view.container.querySelector('.diff-add')).toBeTruthy();
});
