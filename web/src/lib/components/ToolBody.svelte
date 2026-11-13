<script lang="ts">
  // A tool result as numbered rows: a diff tints add/del/hunk lines, ANSI keeps its colours, JSON and
  // source take the regex tokenizers in highlight.ts. Plain output stays plain.
  import {
    diffLineKind,
    hasAnsi,
    highlightAnsi,
    highlightCode,
    highlightDiff,
    highlightJson
  } from '$lib/highlight';

  const MAX_LINES = 2000;
  const MAX_CHARS = 200_000;
  const HEADER = /^\s*(\d+)\s*→/;
  const CODE_PATH = /\.(?:ts|tsx|js|jsx|svelte|rs|go|py|sh|css|html|toml|ya?ml|c|h|cpp|java)$/i;

  let {
    text = '',
    path = '',
    language = '',
    start = 1,
    embedded = false
  }: { text?: string; path?: string; language?: string; start?: number; embedded?: boolean } = $props();

  const raw = $derived(String(text ?? ''));
  const isDiff = $derived(
    language === 'diff' || /^(?:diff --git|--- |\+\+\+ |@@ -\d+)/m.test(raw) || /\.(?:diff|patch)$/.test(path)
  );
  const rows = $derived(
    raw.split('\n').map((line) => {
      const hit = HEADER.exec(line);
      return hit ? { no: hit[1], body: line.slice(hit[0].length) } : { no: '', body: line };
    })
  );
  const tooBig = $derived(raw.length > MAX_CHARS || rows.length > MAX_LINES);
  const lang = $derived(tooBig ? '' : language || (isDiff ? 'diff' : CODE_PATH.test(path) ? 'code' : ''));

  function rowClass(body: string): string {
    // Only a diff tints its +/- rows: a markdown list or a source line can start with the same mark.
    if (!isDiff) return '';
    const kind = diffLineKind(body);
    return kind === 'add' ? 'diff-add' : kind === 'del' ? 'diff-del' : kind === 'hunk' ? 'diff-hunk' : '';
  }

  function tokens(body: string) {
    if (isDiff) return highlightDiff(body);
    if (hasAnsi(body)) return highlightAnsi(body);
    if (lang === 'json') return highlightJson(body);
    if (lang) return highlightCode(body);
    return null;
  }
</script>

{#each rows as row, index (index)}
  {@const pieces = tokens(row.body)}
  <div class="flex gap-2 whitespace-pre-wrap font-mono text-xs leading-5 {rowClass(row.body)}" data-language={lang}>
    <span class="tool-gutter shrink-0 tabular-nums">{row.no || (embedded || !lang ? '' : start + index)}</span>
    <span class="min-w-0 flex-1">
      {#if pieces}{#each pieces as piece, p (p)}{#if piece.cls}<span class={piece.cls}>{piece.text}</span>{:else}{piece.text}{/if}{/each}{:else}{row.body}{/if}
    </span>
  </div>
{/each}
