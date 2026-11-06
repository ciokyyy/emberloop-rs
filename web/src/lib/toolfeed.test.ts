import { describe, it, expect } from 'vitest';
import {
  CHIP_LIMIT,
  bareName,
  bodyKind,
  bodyLabel,
  family,
  paramChips,
  primaryArg,
  resultMeta,
  shortTool,
  visibleChips,
  writtenContent,
} from './toolfeed';

describe('bareName', () => {
  it('drops the MCP and agy prefixes, and leaves a plain name alone', () => {
    expect(bareName('mcp__el__bash')).toBe('bash');
    expect(bareName('mcp__graph__search_graph')).toBe('search_graph');
    expect(bareName('el_read')).toBe('read');
    expect(bareName('Bash')).toBe('Bash');
    expect(bareName('')).toBe('');
  });

  it('shortens a name for the Working indicator', () => {
    expect(shortTool('read')).toBe('read');
    expect(shortTool('get_architecture')).toBe('get_architect…');
    expect(shortTool('  ')).toBe('');
  });
});

const j = (value: unknown) => JSON.stringify(value);

describe('family', () => {
  it('lands pig names, Claude names and MCP-prefixed names on one vocabulary', () => {
    expect(family('bash')).toBe('bash');
    expect(family('Bash')).toBe('bash');
    expect(family('mcp__el__bash')).toBe('bash');
    expect(family('Read')).toBe('read');
    expect(family('MultiEdit')).toBe('edit');
    expect(family('Glob')).toBe('read');
    expect(family('ctx_search')).toBe('ctx_search');
    expect(family('WebSearch')).toBe('websearch');
    expect(family('Task')).toBe('spawn');
  });
  it('reads the one lifecycle tool through its action', () => {
    expect(family('agents', j({ action: 'spawn' }))).toBe('spawn');
    expect(family('Agents', j({ action: 'peek', id: 'c1' }))).toBe('check');
    expect(family('agents', j({ action: 'list' }))).toBe('check');
    expect(family('agents', j({ action: 'steer' }))).toBe('steer');
    expect(family('agents', j({ action: 'cancel' }))).toBe('cancel');
    expect(family('agents')).toBe('check');
  });
  it('falls back to a family by shape and then to other', () => {
    expect(family('mcp__graph__search_graph')).toBe('find');
    expect(family('run_shell_command')).toBe('bash');
    expect(family('quota')).toBe('other');
  });
});

describe('primaryArg', () => {
  it('reads the one argument that names the call', () => {
    expect(primaryArg('bash', j({ command: 'npm test' }))).toBe('npm test');
    expect(primaryArg('Bash', j({ command: 'ls -la' }))).toBe('ls -la');
    expect(primaryArg('read', j({ path: 'src/a.js' }))).toBe('src/a.js');
    expect(primaryArg('Read', j({ file_path: '/tmp/a.js' }))).toBe('/tmp/a.js');
    expect(primaryArg('grep', j({ pattern: 'STRIP_MAX' }))).toBe('STRIP_MAX');
    expect(primaryArg('find', j({ query: 'where the height is decided' }))).toBe('where the height is decided');
    expect(primaryArg('spawn', j({ name: 'pipeline-scout' }))).toBe('pipeline-scout');
    expect(primaryArg('agents', j({ action: 'spawn', agents: [{ prompt: 'job A' }, { prompt: 'job B' }] }))).toBe('job A +1');
    expect(primaryArg('agents', j({ action: 'peek', id: 'c1' }))).toBe('c1');
    expect(primaryArg('agents', j({ action: 'list' }))).toBe('list');
    expect(primaryArg('agents', j({ action: 'cancel', id: 'c2', kill: true }))).toBe('c2');
    expect(primaryArg('ctx_search', j({ queries: ['strip height', 'order'] }))).toBe('"strip height" · "order"');
  });
  it('falls back to the raw input when it is not JSON', () => {
    expect(primaryArg('bash', 'vitest run')).toBe('vitest run');
    expect(primaryArg('read', '')).toBe('');
  });
  it('keeps a long argument to one line', () => {
    const long = j({ command: 'echo ' + 'x'.repeat(200) });
    expect(primaryArg('bash', long).endsWith('…')).toBe(true);
  });
});

describe('paramChips', () => {
  it('lists the arguments that shaped the call', () => {
    expect(paramChips('read', j({ path: 'a.js', offset: 59, limit: 13 }))).toEqual([
      { k: 'offset', v: '59' },
      { k: 'limit', v: '13' },
    ]);
    expect(paramChips('bash', j({ command: 'ls', timeout: 30 }))).toEqual([{ k: 'timeout', v: '30s' }]);
    expect(paramChips('grep', j({ pattern: 'x', path: 'src', glob: '*.js' }))).toEqual([
      { k: 'path', v: 'src' },
      { k: 'glob', v: '*.js' },
    ]);
    expect(paramChips('spawn', j({ prompt: 'p', type: 'scout', model: 'sonnet', deliver: 'group' }))).toEqual([
      { k: 'type', v: 'scout' },
      { k: 'model', v: 'sonnet' },
      { k: 'deliver', v: 'group' },
    ]);
  });
  it('keeps the identifying argument out of the chips', () => {
    const chips = paramChips('read', j({ path: 'a.js', limit: 5 }));
    expect(chips.map((c) => c.k)).not.toContain('path');
  });
  it('drops defaults and empty values', () => {
    expect(paramChips('grep', j({ pattern: 'x', ignore_case: false, context: 0 }))).toEqual([]);
    expect(paramChips('bash', j({ command: 'ls', background: false }))).toEqual([]);
  });
  it('flags a background shell', () => {
    expect(paramChips('bash', j({ command: 'npm run dev', background: true, run_in_background: false }))).toEqual([
      { k: 'background', v: 'true', tone: 'ok' },
    ]);
  });
  it('returns nothing for a non-JSON input', () => {
    expect(paramChips('bash', 'vitest run')).toEqual([]);
  });
  it('never repeats the execute intent the head already names', () => {
    // A description takes the head, so the intent is a chip beside it.
    expect(paramChips('execute', j({ desc: 'count rows', intent: 'tally every row', code: 'print(1)' }))).toEqual([{ k: 'intent', v: 'tally every row' }]);
    // Without a description the intent is the head, so it is not a chip too.
    expect(paramChips('execute', j({ intent: 'tally every row', code: 'print(1)' }))).toEqual([]);
  });
});

describe('resultMeta', () => {
  it('counts the lines a diff added and removed', () => {
    expect(resultMeta({ name: 'edit', output: '-old\n+new\n+newer' })).toBe('+2 −1');
    expect(resultMeta({ name: 'Write', output: '+a' })).toBe('+1 −0');
    expect(resultMeta({ name: 'edit', output: 'no diff here' })).toBe('');
  });
  it('counts hits for a search and lines for a read', () => {
    expect(resultMeta({ name: 'grep', output: 'a:1:x\nb:2:y' })).toBe('2 hits');
    expect(resultMeta({ name: 'grep', output: 'a:1:x' })).toBe('1 hit');
    expect(resultMeta({ name: 'read', output: '  1│ a\n  2│ b' })).toBe('2 lines');
  });
  it('reports a shell as a duration, and says when it failed', () => {
    expect(resultMeta({ name: 'bash', status: 'completed', output: 'ok', elapsedMs: 1400 })).toBe('1.4s');
    expect(resultMeta({ name: 'bash', status: 'failed', output: 'boom', elapsedMs: 1400 })).toBe('failed · 1.4s');
  });
  it('stays empty when there is nothing to say', () => {
    expect(resultMeta({ name: 'bash', output: '', elapsedMs: 0 })).toBe('');
  });
});

describe('bodyKind', () => {
  it('picks the renderer by family, then by shape', () => {
    expect(bodyKind('edit', '@@ -1 +1 @@\n-a\n+b')).toBe('diff');
    expect(bodyKind('write', 'diff --git a/x b/x\n+a\n-b')).toBe('diff');
    // el prints the changed lines as numbered source after a receipt: that is
    // the file, so it reads in the file renderer's colours.
    expect(bodyKind('edit', '[web/x.svelte#1A2B] updated successfully\n     8→  const a = 1;')).toBe('file');
    expect(bodyKind('write', '[web/x.md#1A2B] written (17 lines)')).toBe('stream');
    expect(bodyKind('grep', 'a:1:x')).toBe('grep');
    expect(bodyKind('read', '  1│ a')).toBe('file');
    expect(bodyKind('bash', '$ ls')).toBe('stream');
    expect(bodyKind('mcp__thing__mystery', '{"a":1}')).toBe('json');
    expect(bodyKind('mcp__thing__mystery', 'plain text')).toBe('stream');
  });
});

describe('writtenContent', () => {
  it('reads the file a write carried, and nothing from a command', () => {
    expect(writtenContent('write', j({ path: 'a.md', content: '# hi' }))).toBe('# hi');
    expect(writtenContent('write', j({ path: 'a.md', text: 'x' }))).toBe('x');
    expect(writtenContent('Write', j({ path: 'a.md' }))).toBe('');
    expect(writtenContent('bash', j({ command: 'echo hi', content: 'no' }))).toBe('');
    expect(writtenContent('write', 'not json')).toBe('');
  });
});

describe('visibleChips', () => {
  it('folds past the chip limit and unfolds on demand', () => {
    const chips = Array.from({ length: CHIP_LIMIT + 2 }, (_, i) => ({ k: `k${i}`, v: String(i) }));
    expect(visibleChips(chips)).toEqual({ shown: chips.slice(0, CHIP_LIMIT), hidden: 2 });
    expect(visibleChips(chips, true)).toEqual({ shown: chips, hidden: 0 });
    expect(visibleChips(chips.slice(0, 2))).toEqual({ shown: chips.slice(0, 2), hidden: 0 });
  });
});

describe('bodyLabel', () => {
  it('names the whole string a body is about, past the head\'s truncation', () => {
    const long = 'git log --oneline --graph --all --decorate --pretty=format:%h%x20%s';
    expect(bodyLabel('bash', j({ command: long }))).toEqual({ prefix: '$', text: long });
    expect(bodyLabel('execute_bash', j({ cmd: 'ls -la' }))).toEqual({ prefix: '$', text: 'ls -la' });
  });
  it('reads a finished background process as the command it ran', () => {
    expect(bodyLabel('background', j({ command: 'npx vite' }))).toEqual({ prefix: '$', text: 'npx vite' });
  });
  it('names the file, the pattern and the url', () => {
    expect(bodyLabel('read', j({ path: 'src/lib/events.js' }))).toEqual({ prefix: '', text: 'src/lib/events.js' });
    expect(bodyLabel('edit', j({ file_path: 'a.go' }))).toEqual({ prefix: '', text: 'a.go' });
    expect(bodyLabel('grep', j({ pattern: 'feature line' }))).toEqual({ prefix: '', text: '/feature line/' });
    expect(bodyLabel('web_fetch', j({ url: 'https://example.test/a' }))).toEqual({ prefix: '', text: 'https://example.test/a' });
    expect(bodyLabel('git', j({ args: 'status --short' }))).toEqual({ prefix: '$', text: 'git status --short' });
  });
  it('names nothing when the call names nothing', () => {
    expect(bodyLabel('read', j({}))).toBeNull();
    expect(bodyLabel('mystery', j({ anything: 1 }))).toBeNull();
    expect(bodyLabel(undefined, '')).toBeNull();
  });
});

describe('el edit patch', () => {
  it('names an edit by the path in its hashline header', () => {
    expect(primaryArg('mcp__el__edit', j({ input: '[src/main.rs#AB12]\nPUT 1.=1:\n+x' }))).toBe('src/main.rs');
    expect(primaryArg('mcp__el__edit', j({ path: 'a.rs' }))).toBe('a.rs');
  });
});
