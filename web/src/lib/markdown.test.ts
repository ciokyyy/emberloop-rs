import { describe, expect, it } from 'vitest';
import { renderMarkdown } from './markdown';

describe('markdown', () => {
  it('renders markdown and inline code', () => {
    expect(renderMarkdown('**bold** and `code`')).toContain('<strong>bold</strong>');
    expect(renderMarkdown('**bold** and `code`')).toContain('<code>code</code>');
  });

  it('strips event handlers from raw HTML', () => {
    expect(renderMarkdown('<img src=x onerror="alert(1)">')).not.toContain('onerror');
  });
});
