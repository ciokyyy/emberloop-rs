// Tool output coloring: a tiny regex tokenizer, no dependency. Each function
// takes one line and returns [{ text, cls }] segments; cls is a `tk-*` class
// styled in app.css (empty cls means plain text). Lines are tokenized alone,
// so streaming output colors as it arrives. Joined text always equals the
// input line.

const KEYWORDS = new Set(
  (
    'func function const let var return if else for while switch case break continue import export from as default class extends new ' +
    'this async await try catch finally throw type interface struct enum package go defer range map chan select def lambda pass yield ' +
    'with in is not and or None True False true false null nil undefined void static public private fn pub use mod impl trait match mut'
  ).split(' '),
);

const seg = (text: any, cls: any = '') => ({ text, cls });

function push(out: any, text: any, cls: any = '') {
  if (!text) return;
  const last = out[out.length - 1];
  if (last && last.cls === cls) last.text += text;
  else out.push(seg(text, cls));
}

// scan runs the rules in order at each position; first match wins, and
// unmatched characters join the surrounding plain run.
function scan(line: any, rules: any) {
  const out: any[] = [];
  let i = 0;
  while (i < line.length) {
    let hit = null;
    for (const [re, cls] of rules) {
      re.lastIndex = i;
      const m = re.exec(line);
      if (m && m.index === i && m[0].length > 0) {
        hit = [m[0], typeof cls === 'function' ? cls(m[0]) : cls];
        break;
      }
    }
    if (hit) {
      push(out, hit[0], hit[1]);
      i += hit[0].length;
    } else {
      push(out, line[i]);
      i += 1;
    }
  }
  return out;
}

const STRING = /"(?:[^"\\]|\\.)*"?|'(?:[^'\\]|\\.)*'?|`(?:[^`\\]|\\.)*`?/y;
const NUMBER = /\b0x[0-9a-fA-F]+\b|\b\d+(?:\.\d+)?(?:ms|s|MB|KB|GB|%)?\b/y;
const WORD = /[A-Za-z_][A-Za-z0-9_]*/y;
const PATH = /(?:~|\.{1,2})?\/[\w@.\-+/]+|\b[\w\-]+(?:\/[\w@.\-+]+)+\b|\b[\w\-]+\.(?:go|js|ts|svelte|css|json|md|py|sh|txt|toml|yaml|yml|html|sum|mod)\b/y;

const wordClass = (w: any) => (KEYWORDS.has(w) ? 'tk-kw' : '');

// code colors one source line: comments, strings, numbers, keywords, calls.
export function highlightCode(line: any) {
  const m = /^(\s*)(\/\/.*|#.*|--\s.*|\/\*.*|\*.*)$/.exec(line);
  if (m) return [seg(m[1]), seg(m[2], 'tk-comment')].filter((s: any) => s.text);
  return scan(line, [
    [/\/\/.*|\s#\s.*/y, 'tk-comment'],
    [STRING, 'tk-str'],
    [NUMBER, 'tk-num'],
    [/[A-Za-z_]\w*(?=\()/y, (w: any) => (KEYWORDS.has(w) ? 'tk-kw' : 'tk-fn')],
    [WORD, wordClass],
  ]);
}

// highlightJson colors keys, strings, numbers and literals of one JSON line.
export function highlightJson(line: any) {
  const out: any[] = [];
  const re = /("(?:[^"\\]|\\.)*")(\s*:)?|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|\b(true|false|null)\b/g;
  let last = 0;
  let m;
  while ((m = re.exec(line))) {
    push(out, line.slice(last, m.index));
    if (m[1] !== undefined) {
      // Go HTML-escapes & < > inside JSON strings; show the characters.
      push(out, m[1].replace(/\\u(0026|003c|003e)/gi, (_: any, h: any) => String.fromCharCode(parseInt(h, 16))), m[2] ? 'tk-key' : 'tk-str');
      push(out, m[2] ?? '');
    } else if (m[3] !== undefined) push(out, m[3], 'tk-num');
    else push(out, m[4], 'tk-lit');
    last = re.lastIndex;
  }
  push(out, line.slice(last));
  return out;
}

// highlightDiff styles one diff line; `kind` says how the row is tinted.
export function diffLineKind(line: any) {
  if (line.startsWith('+++') || line.startsWith('---') || line.startsWith('diff ') || line.startsWith('index ')) return 'meta';
  if (line.startsWith('@@')) return 'hunk';
  if (line.startsWith('+')) return 'add';
  if (line.startsWith('-')) return 'del';
  return 'ctx';
}

export function highlightDiff(line: any) {
  const kind = diffLineKind(line);
  if (kind === 'hunk') {
    const m = /^(@@[^@]*@@)(.*)$/.exec(line);
    return m ? [seg(m[1], 'tk-hunk'), seg(m[2], 'tk-fn')].filter((s: any) => s.text) : [seg(line, 'tk-hunk')];
  }
  if (kind === 'meta') return [seg(line, 'tk-dim')];
  if (kind === 'add' || kind === 'del') {
    return [seg(line[0], kind === 'add' ? 'tk-add-sign' : 'tk-del-sign'), ...highlightCode(line.slice(1))];
  }
  return highlightCode(line);
}

// stripAnsi removes terminal escape sequences; ansiSegments keeps the basic
// SGR colors so tools that print color still read in color.
const ANSI = /\x1b\[([0-9;]*)m/g;
const stripAnsi = (s: any) => String(s ?? '').replace(/\x1b\[[0-9;]*[A-Za-z]/g, '');
const ANSI_CLS: Record<number, string> = { 31: 'tk-err', 32: 'tk-ok', 33: 'tk-warn', 34: 'tk-path', 35: 'tk-lit', 36: 'tk-hunk', 90: 'tk-dim', 91: 'tk-err', 92: 'tk-ok', 93: 'tk-warn', 94: 'tk-path', 95: 'tk-lit', 96: 'tk-hunk', 1: 'tk-bold' };

export function hasAnsi(line: any) {
  return line.includes('\x1b[');
}

export function highlightAnsi(line: any) {
  const out: any[] = [];
  let cls = '';
  let last = 0;
  let m;
  ANSI.lastIndex = 0;
  while ((m = ANSI.exec(line))) {
    push(out, stripAnsi(line.slice(last, m.index)), cls);
    for (const code of m[1].split(';')) {
      const n = Number(code || 0);
      if (n === 0) cls = '';
      else if (ANSI_CLS[n]) cls = ANSI_CLS[n];
    }
    last = ANSI.lastIndex;
  }
  push(out, stripAnsi(line.slice(last)), cls);
  return out;
}

// decodeJsonEscapes undoes one layer of JSON string escaping on tool output
// that arrives still encoded (Go HTML-escapes & < > as \u0026/\u003c/\u003e,
// so a literal \uXXXX run means the payload was never decoded: real tool
// output never contains that sequence). A fully quoted "..." line parses as
// JSON; an unquoted fragment only decodes when it carries \uXXXX, so real
// backslashes (Windows paths, \d regexes, printf formats) pass through
// untouched. Anything that fails to parse returns unchanged.
export function decodeJsonEscapes(s: any) {
  const t = String(s ?? '');
  if (!t.includes('\\')) return t;
  const trimmed = t.trim();
  if (trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"')) {
    try {
      const v = JSON.parse(trimmed);
      if (typeof v === 'string') return v;
    } catch {
      // Not a JSON string: fall through to the fragment path.
    }
  }
  if (!/\\u[0-9a-fA-F]{4}/.test(t)) return t;
  try {
    const v = JSON.parse(`"${t}"`);
    if (typeof v === 'string') return v;
  } catch {
    // Invalid escapes (a real \d regex, a trailing \) stay as-is.
  }
  return t;
}

// highlightShell colors a command line: program name, flags, strings, paths.
export function highlightShell(line: any) {
  line = decodeJsonEscapes(line);
  let first = true;
  return scan(line, [
    [/#.*/y, 'tk-comment'],
    [STRING, 'tk-str'],
    [/&&|\|\||[|;>]+|2>&1/y, 'tk-op'],
    [/--?[A-Za-z][\w-]*(?:=\S*)?/y, 'tk-flag'],
    [/\$\{?\w+\}?/y, 'tk-lit'],
    [PATH, 'tk-path'],
    [
      /[A-Za-z_][\w.-]*/y,
      (w: any) => {
        const wasFirst = first;
        first = false;
        return wasFirst ? 'tk-fn' : '';
      },
    ],
  ]);
}

// highlightOutput colors plain command output: status words, paths, numbers.
export function highlightOutput(line: any) {
  line = decodeJsonEscapes(line);
  if (hasAnsi(line)) return highlightAnsi(line);
  const trimmed = line.trim();
  if (/^(error|fatal|panic|fail|--- fail|\s*✗|\s*×)\b/i.test(trimmed) || /\b(error|failed|exception|panic)\b[:!]/i.test(trimmed)) {
    return [seg(line, 'tk-err')];
  }
  if (/^(warn|warning)\b/i.test(trimmed) || /\bwarning[:!]/i.test(trimmed)) return [seg(line, 'tk-warn')];
  if (/^(ok|pass|passed|--- pass|\s*✓|done|success)\b/i.test(trimmed)) return [seg(line, 'tk-ok')];
  const gs = /^(\s*)([MADRCU?!]{1,2})(\s+)(\S.*)$/.exec(line);
  if (gs && /^[MADRCU?!]{1,2}$/.test(gs[2]) && gs[4].includes('/') ) {
    const cls = gs[2].includes('D') ? 'tk-err' : gs[2].includes('?') || gs[2].includes('A') ? 'tk-ok' : 'tk-warn';
    return [seg(gs[1]), seg(gs[2], cls), seg(gs[3]), seg(gs[4], 'tk-path')].filter((s: any) => s.text);
  }
  return scan(line, [
    [STRING, 'tk-str'],
    [PATH, 'tk-path'],
    [/\b\d+(?:\.\d+)?(?:ms|s|MB|KB|GB|%)\b/y, 'tk-num'],
  ]);
}

// outputLineRole judges a whole output line by what it says, so the row itself
// can carry the role: a passing test line reads green, a failure carries the
// tint a diff's deleted line uses, a warning or a comment stays quiet. It is
// the fallback beside highlightOutput, which colours the tokens inside a line.
export function outputLineRole(line: any = '') {
  const text = decodeJsonEscapes(String(line ?? ''));
  const trimmed = text.trim();
  if (!trimmed) return '';
  if (/^(ok|PASS|✓|✔|done|success)\b/i.test(trimmed) || /\b(\d+ passed|all tests passed|tests passed)\b/i.test(trimmed)) return 'tool-ln-ok';
  if (/\b(FAIL|FAILED|Error|error:|AssertionError|Traceback|fatal|panic)\b/.test(text)) return 'tool-ln-err';
  if (/^(warn|warning|WARN|deprecated|skipped|SKIP)\b/i.test(trimmed) || /\b(warning|deprecated)\b[:!]/i.test(trimmed)) return 'tool-ln-warn';
  if (/^(#|\/\/|--)/.test(trimmed)) return 'tool-ln-dim';
  return '';
}

// highlightFileLine handles read output: an optional "  12→" or "12:" gutter
// stays dim, the rest reads as code.
export function highlightFileLine(line: any) {
  const m = /^(\s*\d+\s*[→|:]\s?)(.*)$/.exec(line);
  if (m) return [seg(m[1], 'tk-dim'), ...highlightCode(m[2])];
  return highlightCode(line);
}

// highlightGrepLine colors "path:line:match".
export function highlightGrepLine(line: any) {
  const m = /^([^:\s][^:]*)(:)(\d+)(:?)(.*)$/.exec(line);
  if (!m) return [seg(line)];
  return [seg(m[1], 'tk-path'), seg(m[2], 'tk-dim'), seg(m[3], 'tk-warn'), seg(m[4], 'tk-dim'), ...highlightCode(m[5])].filter((s: any) => s.text);
}

// looksJson tells whether a block of lines is JSON (the tool input).
export function looksJson(lines: any) {
  const t = lines.join('\n').trim();
  return (t.startsWith('{') && t.endsWith('}')) || (t.startsWith('[') && t.endsWith(']'));
}
