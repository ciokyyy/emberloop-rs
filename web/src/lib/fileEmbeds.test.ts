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
});

describe('lists', () => {
  it('keeps nested lists inside their item', () => {
    const html = renderMarkdown('- a\n  - b\n- c');
    expect(html).toContain('<ul>');
    expect(html.match(/<ul>/g)).toHaveLength(2);
  });
});
