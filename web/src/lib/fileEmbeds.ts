import { getIcon } from 'material-file-icons';
import { highlightCode, highlightDiff, highlightJson, highlightShell } from './highlight';

// Turns the `file://` links and images of an answer (rewritten to the daemon's file route by
// `renderMarkdown`, each carrying `data-file`) into something to look at: an image inline, an HTML
// page in a sandboxed frame, a text file in a disclosure that fetches on first open.
const IMAGE = /\.(png|jpe?g|gif|webp|avif|bmp|svg)$/i;
const PAGE = /\.html?$/i;
const TEXT =
  /\.(json|jsonl|csv|tsv|txt|log|md|markdown|ya?ml|toml|xml|ini|env|m?js|tsx?|jsx|css|rs|py|go|sh|sql|svelte|diff|patch)$/i;
const MAX_TEXT = 200_000;

const name = (path: string) => decodeURIComponent(path.split('#')[0].split('/').pop() ?? path);

// The tokenizer already colors tool output; a text file picks its per-line rules by extension.
const highlighter = (path: string) => {
  if (/\.(json|jsonl)$/i.test(path)) return highlightJson;
  if (/\.(diff|patch)$/i.test(path)) return highlightDiff;
  if (/\.(sh|bash|zsh)$/i.test(path)) return highlightShell;
  return highlightCode;
};

function paint(body: HTMLPreElement, text: string, path: string) {
  const color = highlighter(path);
  const fragment = document.createDocumentFragment();
  for (const line of text.split('\n')) {
    for (const part of color(line)) {
      const span = document.createElement('span');
      if (part.cls) span.className = part.cls;
      span.textContent = part.text;
      fragment.append(span);
    }
    fragment.append('\n');
  }
  body.replaceChildren(fragment);
}

function unavailable(path: string) {
  const note = document.createElement('p');
  note.className = 'text-fg-3 text-[12.5px]';
  note.textContent = `File unavailable: ${name(path)}`;
  return note;
}
function fileIcon(path: string) {
  const { name: iconName, svg } = getIcon(name(path));
  const icon = document.createElement('span');
  icon.className = 'md-file-icon';
  icon.setAttribute('aria-hidden', 'true');
  icon.dataset.fileIcon = iconName;
  icon.innerHTML = svg;
  return icon;
}

function textFile(link: HTMLAnchorElement, path: string, label: string) {
  const box = document.createElement('details');
  box.className = 'md-file';
  const title = document.createElement('summary');
  title.append(fileIcon(path), label);
  const body = document.createElement('pre');
  box.append(title, body);
  box.addEventListener(
    'toggle',
    async () => {
      if (!box.open) return;
      try {
        const response = await fetch(link.href);
        if (!response.ok) throw new Error(String(response.status));
        const text = await response.text();
        paint(body, text.length > MAX_TEXT ? `${text.slice(0, MAX_TEXT)}\n… truncated` : text, path);
      } catch {
        box.replaceWith(unavailable(path));
      }
    },
    { once: true }
  );
  return box;
}

function pageFrame(link: HTMLAnchorElement, path: string) {
  const frame = document.createElement('iframe');
  frame.src = link.href;
  frame.title = link.textContent || name(path);
  frame.loading = 'lazy';
  frame.setAttribute('sandbox', ''); // no scripts, no daemon-origin access
  frame.className = 'md-frame';
  return frame;
}

function image(link: HTMLAnchorElement, path: string) {
  const picture = document.createElement('img');
  picture.src = link.href;
  picture.alt = link.textContent || name(path);
  picture.setAttribute('loading', 'lazy');
  picture.addEventListener('error', () => picture.replaceWith(unavailable(path)), { once: true });
  const wrap = document.createElement('a');
  wrap.href = link.href;
  wrap.append(picture);
  return wrap;
}

export function embedFiles(root: HTMLElement) {
  // Text files and pages collect below the answer: a card in the middle of a sentence splits it
  // and leaves its punctuation dangling. Images stay inline where they were written.
  for (const old of root.querySelectorAll(':scope > .md-files')) old.remove();
  const shelf = document.createElement('div');
  shelf.className = 'md-files';
  const grouped = new Map<
    string,
    { link: HTMLAnchorElement; path: string; kind: 'page' | 'text'; labels: Set<string> }
  >();
  for (const link of root.querySelectorAll<HTMLAnchorElement>('a[data-file]')) {
    const rawPath = link.dataset.file ?? '';
    const filePath = rawPath.split('#')[0];
    // Everything stays a link as well, so a file that does not embed still opens in a new tab.
    link.target = '_blank';
    link.rel = 'noopener';
    if (IMAGE.test(filePath)) {
      const embed = image(link, filePath);
      link.after(embed);
      link.remove();
      continue;
    }
    const kind = PAGE.test(filePath) ? 'page' : TEXT.test(filePath) ? 'text' : null;
    if (!kind) continue;
    const label = (link.textContent ?? '').trim() || name(filePath);
    const existing = grouped.get(filePath);
    if (existing) {
      existing.labels.add(label);
    } else {
      grouped.set(filePath, { link, path: filePath, kind, labels: new Set([label]) });
    }
  }
  const embeds: HTMLElement[] = [];
  for (const entry of grouped.values()) {
    const label = entry.labels.size > 1 ? name(entry.path) : ([...entry.labels][0] || name(entry.path));
    const embed =
      entry.kind === 'page' ? pageFrame(entry.link, entry.path) : textFile(entry.link, entry.path, label);
    embeds.push(embed);
  }
  if (embeds.length > 3) {
    const group = document.createElement('details');
    group.className = 'md-files-group';
    const summary = document.createElement('summary');
    const names = [...grouped.values()].map((entry) => name(entry.path));
    const preview = names.slice(0, 3).join(', ') + (names.length > 3 ? ', …' : '');
    summary.textContent = `${embeds.length} files · ${preview}`;
    const list = document.createElement('div');
    list.className = 'md-files-list';
    list.append(...embeds);
    group.append(summary, list);
    shelf.append(group);
  } else if (embeds.length) {
    shelf.append(...embeds);
  }
  if (shelf.childElementCount) root.append(shelf);
  for (const img of root.querySelectorAll<HTMLImageElement>('img[data-file]')) {
    if (img.dataset.embedded) continue;
    img.dataset.embedded = '1';
    img.setAttribute('loading', 'lazy');
    img.addEventListener('error', () => img.replaceWith(unavailable(img.dataset.file ?? '')), {
      once: true
    });
  }
}

/** Svelte action: re-embeds whenever the rendered HTML changes. */
export function fileEmbeds(node: HTMLElement, _html?: string) {
  embedFiles(node);
  return { update: () => embedFiles(node) };
}
