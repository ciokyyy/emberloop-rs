import DOMPurify from 'dompurify';
import { marked } from 'marked';

marked.setOptions({ gfm: true, breaks: true });

export function renderMarkdown(value: string): string {
  const html = marked.parse(value ?? '', { async: false }) as string;
  // A table sits in its own frame: a bare table cannot round its corners with collapsed borders,
  // and the frame is what scrolls a wide one sideways instead of squeezing its columns thin.
  return DOMPurify.sanitize(html, { USE_PROFILES: { html: true } })
    .replaceAll('<table>', '<div class="md-table"><table>')
    .replaceAll('</table>', '</table></div>');
}
