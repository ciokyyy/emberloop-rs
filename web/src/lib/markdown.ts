import DOMPurify from 'dompurify';
import { marked } from 'marked';

marked.setOptions({ gfm: true, breaks: true });

/**
 * `fileBase` is the daemon's file route for the Session (`/api/sessions/<id>/file`): a `file://`
 * link or image in the answer is pointed at it, and keeps the original path in `data-file` for
 * `fileEmbeds`. Without it `file:` URLs stay as the sanitiser leaves them: dropped.
 */
export function renderMarkdown(value: string, fileBase = ''): string {
  let html = marked.parse(value ?? '', { async: false }) as string;
  if (fileBase) {
    html = html.replace(
      /\b(href|src)="file:\/\/([^"]*)"/g,
      (_all, attr: string, path: string) => `${attr}="${fileBase}?path=${path}" data-file="${path}"`
    );
  }
  // A table sits in its own frame: a bare table cannot round its corners with collapsed borders,
  // and the frame is what scrolls a wide one sideways instead of squeezing its columns thin.
  return DOMPurify.sanitize(html, { USE_PROFILES: { html: true } })
    .replaceAll('<table>', '<div class="md-table"><table>')
    .replaceAll('</table>', '</table></div>');
}
