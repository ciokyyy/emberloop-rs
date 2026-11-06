// Tool feed helpers: the pure layer that gives every tool call the same
// display contract, whatever harness ran it.
//
//   head   what the call was — tool, the one argument that identifies it, and
//          the result meta (exit, +a/−d, hits, lines, duration)
//   chips  the other arguments that shaped it, only the ones that were passed
//   body   the result in its own shape (the renderer picks that by kind)
//
// Names arrive from two harnesses (pig sends el names, claude sends its own)
// and from MCP servers, so family() normalizes both to one vocabulary before
// anything is read from the input JSON. Free of Svelte, so the tests pin it.
export const CHIP_LIMIT = 3;
export const FEED_TAIL_LINES = 60;

// One row per family; both spellings land on the same entry, so a pig Session
// and a Claude Session render a shell call the same way.
const FAMILIES = {
  bash: ['bash', 'sh', 'shell', 'terminal', 'execute_bash', 'killshell', 'kill_shell'],
  read: ['read', 'cat', 'view', 'ls', 'notebookread', 'notebook_read', 'glob', 'list_dir'],
  edit: ['edit', 'multiedit', 'multi_edit', 'apply_patch', 'applypatch', 'patch', 'str_replace', 'strreplaceeditor'],
  write: ['write', 'create', 'notebookedit', 'notebook_edit', 'create_file'],
  grep: ['grep', 'rg', 'search'],
  find: ['find', 'search_graph', 'trace_path', 'get_file_outline', 'get_architecture', 'get_code_snippet', 'outline'],
  git: ['git'],
  check: ['check'],
  todo: ['todo', 'todos', 'todo_write', 'todowrite'],
  background: ['background'],
  ctx_search: ['ctx_search', 'ctxsearch', 'context'],
  websearch: ['websearch', 'web_search', 'websearch_deepseek'],
  fetch: ['fetch', 'webfetch', 'web_fetch'],
  spawn: ['spawn', 'task', 'agent'],
  steer: ['steer', 'tell'],
  cancel: ['cancel'],
  submit: ['submit', 'finish'],
  execute: ['execute'],
  compress: ['compress', 'fold'],
  decompress: ['decompress', 'restore', 'unfold'],
};

const BY_NAME = new Map();
for (const [family, names] of Object.entries(FAMILIES)) {
  for (const name of names) BY_NAME.set(name, family);
}

// The Daemon's one Subagent lifecycle tool carries its intent in the action
// instead of the name, so a spawn renders as a spawn, a cancel as a cancel,
// and a peek or a list as the child peek it is.
const AGENT_FAMILIES: Record<string, any> = {
  spawn: 'spawn',
  list: 'check',
  peek: 'check',
  steer: 'steer',
  cancel: 'cancel',
};

const SUBSTRING_FAMILIES: [string[], string][] = [
  [['bash', 'shell', 'exec'], 'bash'],
  [['grep', 'search_doc'], 'grep'],
  [['search'], 'find'],
  [['edit', 'patch'], 'edit'],
  [['write', 'create'], 'write'],
  [['read', 'fetch_file'], 'read'],
  [['git'], 'git'],
];

function fallbackFamily(bare: any) {
  for (const [patterns, fam] of SUBSTRING_FAMILIES) {
    if (patterns.some((p: any) => bare.includes(p))) return fam;
  }
  return 'other';
}

// bareName drops the transport prefix, so a head reads `bash`, not `mcp__el__bash`.
export function bareName(name: any = '') {
  const raw = String(name ?? '').trim();
  return raw.replace(/^mcp__[a-z0-9_-]+__/i, '').replace(/^el_+/i, '') || raw;
}

// shortTool trims a tool name to a few characters for the Working indicator.
export function shortTool(name: any = '', max = 14) {
  const text = String(name ?? '').trim();
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

// family maps a raw tool name to the one vocabulary the head and the chips
// read. An MCP prefix, a server suffix and claude's CamelCase all collapse;
// the agents tool resolves through its action.
export function family(name: any = '', input: any = '') {
  const bare = bareName(name).toLowerCase();
  if (bare === 'agents') {
    const { json } = toolInput(input);
    const action = String(json?.action ?? '').trim().toLowerCase();
    return AGENT_FAMILIES[action] ?? 'check';
  }
  return BY_NAME.get(bare) ?? fallbackFamily(bare);
}

// toolInput parses the arguments a tool was called with. Harnesses send JSON;
// anything that does not parse is kept as the raw text so a plain command
// string still has somewhere to come from.
export function toolInput(input: any = '') {
  const text = String(input ?? '').trim();
  if (!text) return { json: null, raw: '' };
  try {
    const value = JSON.parse(text);
    return { json: value && typeof value === 'object' ? value : null, raw: text };
  } catch {
    return { json: null, raw: text };
  }
}

const pick = (obj: any, keys: any) => {
  for (const key of keys) {
    const value = obj?.[key];
    if (value !== undefined && value !== null && value !== '') return value;
  }
  return '';
};

const firstLine = (text: any, max: any = 80) => {
  const line = String(text ?? '').trim().split('\n')[0] ?? '';
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
};

function primarySpawn(json: any) {
  const agents = Array.isArray(json.agents) ? json.agents : [];
  if (agents.length) {
    const first =
      pick(agents[0], ['name', 'description', 'label']) ||
      firstLine(pick(agents[0], ['prompt']), 60);
    return agents.length > 1 ? `${first} +${agents.length - 1}` : first;
  }
  return pick(json, ['name', 'description', 'label']) || firstLine(pick(json, ['prompt']), 60);
}

function primaryCtx(json: any, raw: any) {
  const queries = json.queries ?? json.query;
  const list = Array.isArray(queries) ? queries : queries ? [queries] : [];
  return list.length ? list.map((q: any) => `"${q}"`).join(' · ') : firstLine(raw);
}

// el's edit tool takes one hashline patch whose first line is `[path#TAG]`.
function patchPath(json: any): string {
  for (const key of ['input', 'patch', 'text']) {
    const header = /^\s*\[([^\]#]+)(?:#[0-9A-Fa-f]+)?\]/.exec(String(json?.[key] ?? ''));
    if (header) return header[1].trim();
  }
  return '';
}

const PRIMARY_HANDLERS: Record<string, any> = {
  bash: (json: any) => firstLine(pick(json, ['command', 'cmd', 'script'])),
  read: (json: any) => pick(json, ['path', 'file_path', 'filePath', 'pattern', 'glob']),
  edit: (json: any) => pick(json, ['path', 'file_path', 'filePath']) || patchPath(json),
  write: (json: any) => pick(json, ['path', 'file_path', 'filePath']),
  grep: (json: any) => pick(json, ['pattern', 'regex', 'query']),
  find: (json: any) => firstLine(pick(json, ['query', 'pattern']), 70),
  git: (json: any) => {
    const args = pick(json, ['args', 'arguments', 'command']);
    return args ? `git ${String(args).trim()}` : 'git';
  },
  websearch: (json: any) => firstLine(pick(json, ['query', 'q']), 70),
  fetch: (json: any) => firstLine(pick(json, ['url', 'href']), 70),
  ctx_search: primaryCtx,
  spawn: primarySpawn,
  background: (json: any) =>
    pick(json, ['command']) ||
    pick(json, ['id', 'session_id', 'sessionId', 'pane', 'target']) ||
    pick(json, ['action']),
  steer: (json: any) => pick(json, ['id', 'session_id', 'sessionId', 'pane', 'target']) || pick(json, ['action']),
  cancel: (json: any) => pick(json, ['id', 'session_id', 'sessionId', 'pane', 'target']) || pick(json, ['action']),
  check: (json: any) => pick(json, ['id', 'session_id', 'sessionId', 'pane', 'target']) || pick(json, ['action']),
  submit: (json: any) => firstLine(pick(json, ['status', 'summary', 'report']), 70),
  todo: (json: any) => pick(json, ['action']),
  execute: (json: any) => firstLine(pick(json, ['desc', 'description', 'intent']) || pick(json, ['code']), 70),
  compress: (json: any, raw: any) => pick(json, ['summary', 'range', 'blockId']) || json.toolAction || firstLine(raw),
  decompress: (json: any, raw: any) => {
    const id = pick(json, ['blockId', 'block_id']);
    return id ? `block ${id}` : json.toolAction || firstLine(raw);
  },
};

// parseMcpName splits a harness MCP tool name (mcp__server__tool or
// mcp_server_tool) into its server and tool, or returns null.
export function parseMcpName(name: any = '') {
  const m = /^(?:mcp__([a-z0-9_-]+)__|mcp_([a-z0-9_-]+?)_)(.+)$/i.exec(String(name));
  return m ? { server: m[1] || m[2], tool: m[3] } : null;
}

// primaryArg is the one argument that names the call: the path for a file
// tool, the command for a shell, the pattern for a grep, the query for a
// search, the Subagent's name for a spawn. It is the only argument the head
// shows, so the row reads the same across every family.
export function primaryArg(name: any = '', input: any = '') {
  const fam = family(name, input);
  const { json, raw } = toolInput(input);
  if (!json) return firstLine(raw);
  const mcp = parseMcpName(name);
  if (json.toolAction && mcp && mcp.server !== 'el') return json.toolAction;
  const handler = PRIMARY_HANDLERS[fam];
  if (handler) return handler(json, raw);
  if (json.toolAction) return json.toolAction;
  return firstLine(
    pick(json, ['path', 'command', 'query', 'pattern', 'name', 'url', 'text']) || raw,
    70,
  );
}

// bodyLabel is what a body's own bar names, in full: the command a shell ran,
// the file a read opened, the pattern a search asked for, the url a fetch
// hit. primaryArg stays the truncated head; this one is the whole string the
// body is about, so the bar never repeats the row's ellipsis.
const textVal = (value: any) => (value === undefined || value === null || value === '' ? '' : String(value).trim());

function editBodyLabel(json: any, output: any) {
  const path = textVal(pick(json, ['path', 'file_path', 'filePath']));
  if (path) return { prefix: '', text: path };
  const named = /^\[([^\]#]+)#/.exec(String(output ?? ''));
  return named ? { prefix: '', text: named[1] } : null;
}

const BODY_HANDLERS: Record<string, any> = {
  bash: (json: any) => {
    const cmd = textVal(pick(json, ['command', 'cmd', 'script']));
    return cmd ? { prefix: '$', text: cmd } : null;
  },
  git: (json: any) => {
    const args = textVal(pick(json, ['args', 'arguments', 'command']));
    return { prefix: '$', text: args ? `git ${args}` : 'git' };
  },
  background: (json: any) => {
    const cmd = textVal(pick(json, ['command']));
    return cmd ? { prefix: '$', text: cmd } : null;
  },
  read: (json: any, out: any) => editBodyLabel(json, out),
  edit: (json: any, out: any) => editBodyLabel(json, out),
  write: (json: any, out: any) => editBodyLabel(json, out),
  grep: (json: any) => {
    const pattern = textVal(pick(json, ['pattern', 'regex', 'query']));
    return pattern ? { prefix: '', text: `/${pattern}/` } : null;
  },
  fetch: (json: any) => {
    const url = textVal(pick(json, ['url', 'href']));
    return url ? { prefix: '', text: url } : null;
  },
  websearch: (json: any) => {
    const query = textVal(pick(json, ['query', 'q']));
    return query ? { prefix: '', text: query } : null;
  },
};

// bodyLabel is what a body's own bar names, in full: the command a shell ran,
// the file a read opened, the pattern a search asked for, the url a fetch
// hit. primaryArg stays the truncated head; this one is the whole string the
// body is about, so the bar never repeats the row's ellipsis.
export function bodyLabel(name: any = '', input: any = '', output: any = '') {
  const fam = family(name, input);
  const { json, raw } = toolInput(input);
  if (!json) return raw ? { prefix: '', text: raw } : null;
  const handler = BODY_HANDLERS[fam];
  return handler ? handler(json, output) : null;
}

// paramChips are the arguments that shaped the call but are not its identity.
// Only what the caller actually passed appears, and defaults stay out, so a
// bare `read path.go` never grows a row of noise.
const PARAM_HANDLERS: Record<string, any> = {
  bash: (json: any, { add, yes }: any) => {
    add('timeout', json.timeout, typeof json.timeout === 'number' ? 's' : '');
    yes('background', json.background ?? json.run_in_background);
    add('description', json.description);
  },
  read: (json: any, { add }: any) => {
    add('offset', json.offset);
    add('limit', json.limit);
    add('glob', json.glob);
  },
  grep: (json: any, { add, chips }: any) => {
    add('path', json.path);
    add('glob', json.glob);
    add('context', json.context);
    add('max_results', json.max_results);
    if (json.ignore_case === true) chips.push({ k: 'ignore_case', v: 'true', tone: 'ok' });
    if (json.fixed === true) chips.push({ k: 'fixed', v: 'true', tone: 'ok' });
  },
  find: (json: any, { add }: any) => {
    add('path', json.path);
    add('mode', json.mode);
    add('depth', json.depth);
  },
  websearch: (json: any, { add, chips }: any) => {
    add('count', json.count);
    add('freshness', json.freshness);
    add('provider', json.provider);
    if (Array.isArray(json.domains) && json.domains.length) {
      chips.push({ k: 'domains', v: json.domains.slice(0, 2).join(', ') });
    }
  },
  ctx_search: (json: any, { add, chips }: any) => {
    const queries = json.queries;
    if (Array.isArray(queries) && queries.length > 1) chips.push({ k: 'queries', v: String(queries.length) });
    add('source', json.source);
    add('limit', json.limit);
  },
  git: (json: any, { add }: any) => {
    add('timeout', json.timeout, typeof json.timeout === 'number' ? 's' : '');
  },
  spawn: (json: any, { add }: any) => {
    const agents = Array.isArray(json.agents) ? json.agents : [];
    if (agents.length > 1) add('agents', agents.length);
    add('type', json.type ?? pick(agents[0] ?? {}, ['type']));
    add('model', json.model);
    add('deliver', json.deliver);
  },
  steer: (json: any, { add, chips }: any) => {
    add('state', json.state);
    add('queue', json.queue);
    if (json.kill === true) chips.push({ k: 'kill', v: 'true', tone: 'warn' });
  },
  cancel: (json: any, { add, chips }: any) => {
    add('state', json.state);
    add('queue', json.queue);
    if (json.kill === true) chips.push({ k: 'kill', v: 'true', tone: 'warn' });
  },
  check: (json: any, { add }: any) => {
    add('state', json.state);
    add('queue', json.queue);
  },
  background: (json: any, { add }: any) => {
    add('state', json.state);
    add('queue', json.queue);
  },
  execute: (json: any, { add }: any) => {
    add('timeout', json.timeout, typeof json.timeout === 'number' ? 's' : '');
    if (pick(json, ['desc', 'description']) !== '') add('intent', json.intent);
  },
};

// paramChips are the arguments that shaped the call but are not its identity.
// Only what the caller actually passed appears, and defaults stay out, so a
// bare `read path.go` never grows a row of noise.
export function paramChips(name: any = '', input: any = '') {
  const fam = family(name, input);
  const { json } = toolInput(input);
  if (!json) return [];
  const chips: any[] = [];
  const add = (k: any, value: any, suffix: any = '') => {
    if (value === undefined || value === null || value === '' || value === false || value === 0) return;
    chips.push({ k, v: `${value}${suffix}` });
  };
  const yes = (k: any, value: any, label: any = 'true') => {
    if (value === true) chips.push({ k, v: label, tone: 'ok' });
  };
  const handler = PARAM_HANDLERS[fam];
  if (handler) handler(json, { add, yes, chips });
  return chips;
}

// resultMeta is the quiet number at the right edge of the head: what the call
// produced, in the unit that call is judged by.
function formatElapsed(elapsedMs: any) {
  if (elapsedMs <= 0) return '';
  if (elapsedMs >= 60000) {
    return `${Math.floor(elapsedMs / 60000)}m${Math.round((elapsedMs % 60000) / 1000)}s`;
  }
  return `${(elapsedMs / 1000).toFixed(1)}s`;
}

function diffMeta(lines: any) {
  let add = 0;
  let del = 0;
  for (const line of lines) {
    if (line.startsWith('+')) add += 1;
    else if (line.startsWith('-')) del += 1;
  }
  return add || del ? `+${add} −${del}` : null;
}

function readMeta(lines: any) {
  const count = lines.filter((line: any, i: any) => line !== '' || i < lines.length - 1).length;
  return count ? `${count} line${count === 1 ? '' : 's'}` : null;
}

function searchMeta(lines: any) {
  const hits = lines.filter((line: any) => line.trim()).length;
  return hits ? `${hits} hit${hits === 1 ? '' : 's'}` : null;
}

const META_FORMATTERS: Record<string, any> = {
  edit: diffMeta,
  write: diffMeta,
  read: readMeta,
  grep: searchMeta,
  find: searchMeta,
};

function contentMeta(fam: any, lines: any) {
  const fmt = META_FORMATTERS[fam];
  return fmt ? fmt(lines) : null;
}

// resultMeta is the quiet number at the right edge of the head: what the call
// produced, in the unit that call is judged by.
export function resultMeta({ name = '', status = '', output = '', elapsedMs = 0 }: any = {}) {
  const fam = family(name);
  const lines = String(output ?? '').split('\n');
  const content = contentMeta(fam, lines);
  if (content) return content;
  const failed = String(status ?? '').toLowerCase() === 'failed';
  const seconds = formatElapsed(elapsedMs);
  const exit = failed ? 'failed · ' : '';
  return `${exit}${seconds}`.trim();
}

// looksDiff tells a unified diff from a file the agent just wrote: a real diff
// carries hunk headers and signed lines, while the result of a write is the
// file itself (numbered, since el hands back the lines it changed).
export function looksDiff(text: any = '') {
  const body = String(text ?? '');
  if (/^(?:diff --git |@@ )/m.test(body)) return true;
  return /^\+/m.test(body) && /^-/m.test(body);
}

// numberedSource tells a result that is a file the tool printed with its own
// line numbers ("  40→  const x = 1", the shape el and claude both use).
export function numberedSource(text: any = '') {
  return /^\s*\d+\s*→/m.test(String(text ?? ''));
}

// bodyKind names the renderer for a result: a diff wants red and green, a
// shell wants a terminal, a grep wants marked hits, and the rest is JSON or
// plain output. An edit or a write is judged by what it printed: a patch is a
// diff, the file it wrote is source (so its lines are coloured by the same
// tokenizer a read uses), and a bare receipt stays plain text.
export function bodyKind(name: any = '', output: any = '') {
  const fam = family(name);
  const text = String(output ?? '');
  if (fam === 'edit' || fam === 'write') {
    if (looksDiff(text)) return 'diff';
    return numberedSource(text) ? 'file' : 'stream';
  }
  if (fam === 'grep') return 'grep';
  if (fam === 'read') return 'file';
  if (fam === 'bash' || fam === 'git' || fam === 'check') return 'stream';
  if (fam === 'find' || fam === 'ctx_search' || fam === 'websearch') return 'stream';
  const trimmed = text.trim();
  if (trimmed.startsWith('{') || trimmed.startsWith('[')) return 'json';
  return 'stream';
}

// writtenContent is the file a write-family call carried in its arguments: the
// tool's own result is only a receipt ("written (17 lines)"), so the body shows
// what it wrote, coloured the way a read of that file would be.
export function writtenContent(name: any = '', input: any = '') {
  const { json } = toolInput(input);
  if (!json) return '';
  const fam = family(name, input);
  if (fam !== 'write') return '';
  const content = json.content ?? json.text ?? json.body;
  return typeof content === 'string' ? content : '';
}

// visibleChips applies the fold: past three arguments the row shows three and
// a count, because eight chips is a wall of grey on a phone.
export function visibleChips(chips: any = [], expanded: any = false) {
  if (expanded || chips.length <= CHIP_LIMIT) return { shown: chips, hidden: 0 };
  return { shown: chips.slice(0, CHIP_LIMIT), hidden: chips.length - CHIP_LIMIT };
}

// executeCode extracts the Python code from an execute tool input payload.
// Supports both direct {"code": ...} and nested {"arguments": {"code": ...}} shapes.
export function executeCode(input: any = '') {
  const { json } = toolInput(input);
  if (!json) return '';
  const code = json.code ?? json.arguments?.code;
  return typeof code === 'string' ? code : '';
}

// bashSingleFileView returns the single target file path if the bash run was classified
// as a single-file read/view operation, otherwise empty string.
export function bashSingleFileView(run: any = {}) {
  const kind = run?.display?.kind;
  if (kind !== 'read') return '';
  const parts = run?.display?.parts;
  if (!Array.isArray(parts) || parts.length !== 1) return '';
  return typeof parts[0]?.target === 'string' ? parts[0].target : '';
}
