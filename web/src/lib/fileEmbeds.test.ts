import { describe, expect, it, vi } from 'vitest';
import { embedFiles } from './fileEmbeds';
import { renderMarkdown } from './markdown';

const BASE = '/api/sessions/s1/file';
const mount = (markdown: string) => {
  const root = document.createElement('div');
  root.innerHTML = renderMarkdown(markdown, BASE);
  embedFiles(root);
  return root;
};

describe('file links', () => {
  it('points a file link at the Session file route', () => {
    const html = renderMarkdown('[a](file:///work/my%20doc.json)', BASE);
    expect(html).toContain(`href="${BASE}?path=/work/my%20doc.json"`);
    expect(html).toContain('data-file="/work/my%20doc.json"');
  });

  it('drops file links when no Session route is given', () => {
    expect(renderMarkdown('[a](file:///etc/passwd)')).not.toContain('file:');
  });

  it('shows an image inline', () => {
    const img = mount('![shot](file:///work/a.png)').querySelector('img');
    expect(img?.getAttribute('src')).toBe(`${BASE}?path=/work/a.png`);
    expect(img?.getAttribute('loading')).toBe('lazy');
  });

  it('puts an HTML file below the answer in a sandboxed frame without scripts', () => {
    const frame = mount('[page](file:///work/p.html)').querySelector('.md-files iframe');
    expect(frame?.getAttribute('sandbox')).toBe('');
  });

  it('stacks a text file below the answer and keeps its link in the sentence', () => {
    const root = mount('[cfg](file:///work/c.json) [bin](file:///work/x.exe)');
    expect(root.querySelector('details.md-file summary')?.textContent).toBe('cfg');
    expect(root.querySelector('.md-files details')).not.toBeNull();
    expect(root.querySelector('a[data-file$="c.json"]')?.textContent).toBe('cfg');
    expect(root.querySelector('a[data-file$="x.exe"]')?.getAttribute('target')).toBe('_blank');
  });

  it('colors an opened text file by its extension', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true, text: async () => '{"a": 1}' })));
    const box = mount('[cfg](file:///work/c.json)').querySelector('details')!;
    box.open = true;
    box.dispatchEvent(new Event('toggle'));
    await vi.waitFor(() => expect(box.querySelector('.tk-key')?.textContent).toBe('"a"'));
    vi.unstubAllGlobals();
  });

  it('deduplicates multiple links to the same file and groups many files', () => {
    const repeated = mount(
      '[fnA](file:///work/shared.rs#L1-L5) [fnB](file:///work/shared.rs#L10-L20)'
    );
    const boxes = repeated.querySelectorAll('details.md-file');
    expect(boxes).toHaveLength(1);
    expect(boxes[0]?.querySelector('summary')?.textContent).toBe('shared.rs');

    const many = mount(
      '[a](file:///work/a.rs) [b](file:///work/b.rs) [c](file:///work/c.rs) [d](file:///work/d.rs)'
    );
    const group = many.querySelector('details.md-files-group');
    expect(group).not.toBeNull();
    expect(group?.querySelector('summary')?.textContent).toBe('4 files · a.rs, b.rs, c.rs, …');
    expect(group?.querySelectorAll('details.md-file')).toHaveLength(4);
  });

  it('shows a filetype icon by extension in each file disclosure', () => {
    const root = mount('[lib](file:///work/lib.rs) [view](file:///work/Feed.svelte)');
    const icons = [...root.querySelectorAll('.md-file summary .md-file-icon')];
    expect(icons.map((el) => (el as HTMLElement).dataset.fileIcon)).toEqual(['rust', 'svelte']);
    expect(icons.every((el) => el.querySelector('svg') !== null)).toBe(true);
    expect(root.querySelectorAll('.md-file summary')[0]?.textContent).toBe('lib');
  });

  it('replaces the file shelf without duplicating rows across streaming re-renders', () => {
    const root = document.createElement('div');
    root.innerHTML = renderMarkdown('[tokens](file:///work/tokens.rs)', BASE);
    embedFiles(root);
    // Svelte {@html} replaces the markdown nodes while keeping manually appended siblings.
    const shelf = root.querySelector('.md-files')!;
    const firstParagraph = root.firstElementChild!;
    const next = document.createElement('div');
    next.innerHTML = renderMarkdown('[tokens](file:///work/tokens.rs) [acp](file:///work/acp.rs)', BASE);
    firstParagraph.replaceWith(...next.childNodes);
    embedFiles(root);
    embedFiles(root);
    expect(root.querySelectorAll('.md-files')).toHaveLength(1);
    expect(root.querySelectorAll('details.md-file')).toHaveLength(2);
  });
});

describe('lists', () => {
  it('keeps nested lists inside their item', () => {
    const html = renderMarkdown('- a\n  - b\n- c');
    expect(html).toContain('<ul>');
    expect(html.match(/<ul>/g)).toHaveLength(2);
  });
});
