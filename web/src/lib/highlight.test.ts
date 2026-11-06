import { describe, it, expect } from 'vitest';
import { highlightCode, highlightJson, highlightDiff, diffLineKind, highlightShell, highlightOutput, highlightGrepLine, highlightFileLine, highlightAnsi, looksJson, decodeJsonEscapes, outputLineRole } from './highlight';

const text = (segs: any[]) => segs.map((s: any) => s.text).join('');
const cls = (segs: any[], t: string) => segs.find((s: any) => s.text === t)?.cls;

describe('highlight', () => {
  it('keeps the line text intact', () => {
    for (const fn of [highlightCode, highlightJson, highlightDiff, highlightShell, highlightOutput, highlightGrepLine, highlightFileLine, highlightAnsi]) {
      for (const l of ['const x = "a b" // hi', '  {"k": [1, true, null]}', '+added line', 'git status --short && ls ~/x', 'a.go:12:func main()', '  12→ return 1', '']) {
        expect(text(fn(l))).toBe(l);
      }
    }
  });
  it('colors code tokens', () => {
    const s = highlightCode('return foo("x", 42)');
    expect(cls(s, 'return')).toBe('tk-kw');
    expect(cls(s, 'foo')).toBe('tk-fn');
    expect(cls(s, '"x"')).toBe('tk-str');
    expect(cls(s, '42')).toBe('tk-num');
  });
  it('colors json keys apart from strings', () => {
    const s = highlightJson('  "path": "/a/b",');
    expect(cls(s, '"path"')).toBe('tk-key');
    expect(cls(s, '"/a/b"')).toBe('tk-str');
  });
  it('classifies diff lines', () => {
    expect(diffLineKind('+x')).toBe('add');
    expect(diffLineKind('-x')).toBe('del');
    expect(diffLineKind('+++ b/x')).toBe('meta');
    expect(diffLineKind('@@ -1 +1 @@')).toBe('hunk');
    expect(highlightDiff('+  return 1')[0].cls).toBe('tk-add-sign');
  });
  it('colors shell commands', () => {
    const s = highlightShell('git commit -m "x" && ls');
    expect(cls(s, 'git')).toBe('tk-fn');
    expect(cls(s, '-m')).toBe('tk-flag');
    expect(cls(s, '"x"')).toBe('tk-str');
    expect(cls(s, '&&')).toBe('tk-op');
  });
  it('colors output status and ansi', () => {
    expect(highlightOutput('FAIL example.com/x')[0].cls).toBe('tk-err');
    expect(highlightOutput('ok  example.com/x 0.1s')[0].cls).toBe('tk-ok');
    expect(highlightAnsi('\x1b[31mred\x1b[0m plain')[0]).toEqual({ text: 'red', cls: 'tk-err' });
  });
  it('colors grep and file lines', () => {
    const g = highlightGrepLine('src/a.js:12:const x = 1');
    expect(g[0]).toEqual({ text: 'src/a.js', cls: 'tk-path' });
    expect(highlightFileLine('  12→ const a = 1')[0].cls).toBe('tk-dim');
  });
  it('detects json blocks', () => {
    expect(looksJson(['{', '"a": 1', '}'])).toBe(true);
    expect(looksJson(['hello'])).toBe(false);
  });
  it('decodes JSON-escaped tool output before tokenizing', () => {
    expect(decodeJsonEscapes('git commit \\u0026\\u0026 ls')).toBe('git commit && ls');
    expect(decodeJsonEscapes('2\\u003e\\u00261')).toBe('2>&1');
    expect(decodeJsonEscapes('"a\\nb"')).toBe('a\nb');
    expect(decodeJsonEscapes('say \\"hi\\" \\u0026 done')).toBe('say "hi" & done');
    const shell = highlightShell('git commit \\u0026\\u0026 ls');
    expect(text(shell)).toBe('git commit && ls');
    expect(cls(shell, '&&')).toBe('tk-op');
    const out = highlightOutput('2\\u003e\\u00261');
    expect(text(out)).toBe('2>&1');
  });
  it('leaves real backslashes alone', () => {
    expect(decodeJsonEscapes('C:\\new\\test')).toBe('C:\\new\\test');
    expect(decodeJsonEscapes('grep \\d+ file')).toBe('grep \\d+ file');
    expect(decodeJsonEscapes('printf %s\\n')).toBe('printf %s\\n');
    expect(text(highlightOutput('C:\\new\\test'))).toBe('C:\\new\\test');
  });

  it('shows Go HTML-escaped characters inside JSON string tokens', () => {
    const segs = highlightJson('{"command": "ls \\u0026\\u0026 echo 2\\u003e\\u00261"}');
    expect(segs.map((x) => x.text).join('')).toBe('{"command": "ls && echo 2>&1"}');
  });
});

describe('outputLineRole', () => {
  it('marks a passing line, a failure, a warning and a comment', () => {
    expect(outputLineRole('ok   example.com/web  0.62s')).toBe('tool-ln-ok');
    expect(outputLineRole('PASS src/lib/x.test.js')).toBe('tool-ln-ok');
    expect(outputLineRole('  Tests  12 passed (12)')).toBe('tool-ln-ok');
    expect(outputLineRole('FAIL  src/lib/story.test.js > splits the turn')).toBe('tool-ln-err');
    expect(outputLineRole('AssertionError: expected 2 cards, got 3')).toBe('tool-ln-err');
    expect(outputLineRole('warning: unused variable')).toBe('tool-ln-warn');
    expect(outputLineRole('// a note')).toBe('tool-ln-dim');
    expect(outputLineRole('# a note')).toBe('tool-ln-dim');
  });
  it('leaves an ordinary line without a role', () => {
    expect(outputLineRole('total 48')).toBe('');
    expect(outputLineRole('')).toBe('');
  });
  it('reads the escapes a JSON output string carries', () => {
    expect(outputLineRole('FAIL\\u0020x')).toBe('tool-ln-err');
  });
});
