// Compact feed model: turns a toolstrip block's runs into the compact structure.
// Pure model with no rendering and no side effects.
// Rules from .scratch/tool-feed-clarity/spec.md.

import { family, primaryArg, executeCode, parseMcpName, bareName } from './toolfeed';
import { skillManageView } from './skilldiff';

// parseExitCode extracts the integer exit code from [exit status N] in bash output.
export function parseExitCode(output: any = '') {
  // el's bash tool prints `exit code: N` as its first line; the Go tools print `[exit status N]`.
  const text = String(output ?? '');
  const match = text.match(/\[exit status\s+(\d+)\]/i) ?? text.match(/^exit code:\s*(\d+)/);
  return match ? parseInt(match[1], 10) : null;
}

// parseErrorTail extracts the last two non-empty error lines preceding [exit status N].
export function parseErrorTail(output: any = '') {
  const text = String(output ?? '').trim();
  if (!text) return '';
  const lines = text
    .split('\n')
    .map((l: any) => l.trim())
    .filter((l: any) => l.length > 0 && !/^\[exit status\s+\d+\]$/i.test(l) && !/^exit code:\s*\d+$/.test(l));
  if (lines.length === 0) return '';
  return lines.slice(-2).join('\n');
}

// parseDiffStats counts additions and deletions from a unified diff string.
export function parseDiffStats(diff: any = '', inputPath: any = '') {
  let path = inputPath;
  let additions = 0;
  let deletions = 0;

  if (diff) {
    const lines = String(diff).split('\n');
    for (const line of lines) {
      if (line.startsWith('+++ b/')) {
        if (!path) path = line.slice(6).trim();
      } else if (line.startsWith('+') && !line.startsWith('+++')) {
        additions++;
      } else if (line.startsWith('-') && !line.startsWith('---')) {
        deletions++;
      }
    }
  }

  return { path, additions, deletions };
}

// formatDuration formats elapsed milliseconds as a readable second string (e.g. "12s").
export function formatDuration(ms: any = 0) {
  const s = Math.round((Number(ms) || 0) / 1000);
  return `${Math.max(1, s)}s`;
}

function formatVisibleDuration(ms: any = 0, isRunning = false) {
  const n = Number(ms) || 0;
  if (n <= 0) return isRunning ? '0s' : null;
  if (n < 1000) return isRunning ? '0s' : '<1s';
  if (n >= 60000) return formatTotalTime(n);
  return formatDuration(n);
}
// formatTotalTime formats total elapsed milliseconds into "4m 02s" or "12s".
export function formatTotalTime(ms: any = 0) {
  const totalSec = Math.max(0, Math.round((Number(ms) || 0) / 1000));
  const mins = Math.floor(totalSec / 60);
  const secs = totalSec % 60;
  if (mins > 0) {
    return `${mins}m ${String(secs).padStart(2, '0')}s`;
  }
  return `${secs}s`;
}

// isReadOnly reports whether a run is a successful read-only exploration call.
const READ_ONLY_FAMS = new Set(['read', 'grep', 'find']);

function isReadOnlyFam(fam: any, run: any) {
  if (fam === 'execute') {
    const children = Array.isArray(run.children) ? run.children : [];
    return children.length > 0 && children.every(isReadOnly);
  }
  if (fam === 'bash') {
    const kind = run.display?.kind;
    return kind === 'read' || kind === 'search' || kind === 'list';
  }
  return READ_ONLY_FAMS.has(fam);
}

// isReadOnly reports whether a run is a successful read-only exploration call.
function isReadOnly(run: any) {
  if (!run) return false;
  const mcp = parseMcpName(run.name);
  if (mcp && mcp.server !== 'el') return false;
  const exitCode = parseExitCode(run.output);
  if (run.status === 'failed' || (exitCode !== null && exitCode !== 0)) return false;
  return isReadOnlyFam(family(run.name, run.input), run);
}

// verbForKind capitalizes the call kind for child display.
function verbForKind(kind: any = '') {
  switch (String(kind).toLowerCase()) {
    case 'read':
      return 'Read';
    case 'search':
      return 'Search';
    case 'list':
      return 'List';
    default:
      return 'Read';
  }
}

// getPartsForRun returns the list of parts for a read-only run.
function getBashParts(run: any) {
  let parts = run.display?.parts;
  if (typeof parts === 'string') {
    try {
      parts = JSON.parse(parts);
    } catch {
      parts = null;
    }
  }
  if (Array.isArray(parts) && parts.length > 0) {
    return parts.map((p: any) => ({
      verb: verbForKind(p.kind),
      target: p.target || '.',
      kind: p.kind || 'read',
      run,
    }));
  }
  const fallbackKind = run.display?.kind || 'read';
  return [
    {
      verb: verbForKind(fallbackKind),
      target: primaryArg(run.name, run.input) || '.',
      kind: fallbackKind,
      run,
    },
  ];
}

const STATIC_PARTS: Record<string, any> = {
  read: (run: any) => [{ verb: 'Read', target: primaryArg(run.name, run.input) || 'file', kind: 'read', run }],
  grep: (run: any) => [{ verb: 'Search', target: primaryArg(run.name, run.input) || 'pattern', kind: 'search', run }],
  find: (run: any) => [{ verb: 'Search', target: primaryArg(run.name, run.input) || '.', kind: 'search', run }],
  execute: (run: any) => (Array.isArray(run.children) ? run.children : []).flatMap(getPartsForRun),
};

// getPartsForRun returns the list of parts for a read-only run.
function getPartsForRun(run: any) {
  const fam = family(run.name, run.input);
  if (fam === 'bash') return getBashParts(run);
  const handler = STATIC_PARTS[fam];
  return handler ? handler(run) : [];
}

// formatGroupLabel renders "Explored 2 listings · 1 search · 1 file".
export function formatGroupLabel(counts: any = {}) {
  const listings = counts.list || 0;
  const search = counts.search || 0;
  const files = counts.read || 0;
  const bits = [];

  if (listings > 0) {
    bits.push(`${listings} ${listings === 1 ? 'listing' : 'listings'}`);
  }
  if (search > 0) {
    bits.push(`${search} ${search === 1 ? 'search' : 'searches'}`);
  }
  if (files > 0) {
    bits.push(`${files} ${files === 1 ? 'file' : 'files'}`);
  }

  if (bits.length === 0) return 'Explored';
  return `Explored ${bits.join(' · ')}`;
}

// compactFeed turns a toolstrip block's runs into the compact feed model.
function isRunFailed(run: any) {
  const exitCode = parseExitCode(run.output);
  return run.status === 'failed' || (exitCode !== null && exitCode !== 0);
}

function countTurnRun(run: any, counts: any) {
  if (isRunFailed(run)) counts.failed = true;
  counts.elapsed += Number(run.elapsedMs) || 0;
  if (isReadOnly(run)) {
    counts.explored += getPartsForRun(run).length;
    return;
  }
  const fam = family(run.name, run.input);
  if (fam === 'edit' || fam === 'write') {
    counts.edited++;
  } else if (run.name !== 'thinking' && !run.thinking) {
    counts.ran++;
  }
}

function formatTurnBits(c: any) {
  const bits = [];
  if (c.explored > 0) bits.push(`Explored ${c.explored} ${c.explored === 1 ? 'file' : 'files'}`);
  if (c.ran > 0) bits.push(`Ran ${c.ran} ${c.ran === 1 ? 'command' : 'commands'}`);
  if (c.edited > 0) bits.push(`Edited ${c.edited} ${c.edited === 1 ? 'file' : 'files'}`);
  return bits.join(' · ') || 'Completed turn';
}

function summarizeTurnLine(runs: any, block: any) {
  const c = { explored: 0, ran: 0, edited: 0, failed: false, elapsed: 0 };
  for (const run of runs) countTurnRun(run, c);
  const total = c.elapsed || block?.elapsedMs || 0;
  return [
    {
      type: 'turn-line',
      label: formatTurnBits(c),
      counts: { explored: c.explored, ran: c.ran, edited: c.edited },
      totalTime: formatTotalTime(total),
      elapsedMs: total,
      failed: c.failed,
    },
  ];
}

function appendToGroup(group: any, run: any, isRunning: any, isFailed: any) {
  group.runs.push(run);
  if (isRunning) group.running = true;
  if (isFailed) group.failed = true;
  const parts = getPartsForRun(run);
  for (const p of parts) {
    group.counts[p.kind] = (group.counts[p.kind] || 0) + 1;
    group.children.push(p);
  }
}

function runTitle(run: any) {
  return typeof run?.display?.title === 'string' ? run.display.title.trim() : '';
}

function parseJsonObject(raw: any) {
  if (!raw || typeof raw !== 'string') return null;
  try {
    const v = JSON.parse(raw);
    if (!v || typeof v !== 'object') return null;
    if (v.arguments && typeof v.arguments === 'object' && !Array.isArray(v.arguments)) {
      return { ...v.arguments, ...v };
    }
    return v;
  } catch {
    return null;
  }
}

function executeHeaderComment(code: string) {
  if (!code) return '';
  for (const rawLine of code.split('\n')) {
    const line = rawLine.trim();
    if (!line) continue;
    if (line.startsWith('#')) {
      if (/^#!|^# -\*-|^#\s*(?:type|noqa|pylint|mypy):/i.test(line)) continue;
      const cleaned = line.replace(/^#+\s*/, '').replace(/^(?:description|intent|title)\s*:\s*/i, '').trim();
      if (cleaned) return cleaned.length > 84 ? `${cleaned.slice(0, 83)}…` : cleaned;
      continue;
    }
    if (/^(?:import |from .+ import )/.test(line)) continue;
    break;
  }
  return '';
}

function summarizeCodeCalls(code: string) {
  if (!code) return '';
  const hits: string[] = [];
  const seen = new Set<string>();
  const re = /\btools\.(?:call\(\s*['"]([a-z0-9_.-]+)['"]|([a-z0-9_]+)\s*\()/gi;
  for (const m of code.matchAll(re)) {
    const name = (m[1] || m[2] || '').split('.').pop() || '';
    if (!name || name === 'parallel' || name === 'store' || name === 'load' || name === 'search' || name === 'help') continue;
    if (!seen.has(name)) {
      seen.add(name);
      hits.push(name);
    }
  }
  if (hits.length > 0) {
    const label = hits.slice(0, 3).map((h) => h.replace(/_/g, ' ')).join(', ');
    return hits.length > 3 ? `Script: ${label} +${hits.length - 3}` : `Script: ${label}`;
  }
  for (const rawLine of code.split('\n')) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#') || /^(?:import |from .+ import )/.test(line)) continue;
    return line.length > 72 ? `${line.slice(0, 71)}…` : line;
  }
  return '';
}

function shortTarget(target: string) {
  const t = String(target ?? '').trim();
  if (!t) return '';
  const parts = t.split('/');
  if (parts.length <= 2) return t;
  return parts.slice(-2).join('/');
}

function summarizeChildrenTitle(children: any[] = []) {
  if (!Array.isArray(children) || children.length === 0) return '';
  const groups = new Map<string, { count: number; targets: string[] }>();
  for (const c of children) {
    const fam = family(c.name, c.input);
    const target = shortTarget(c.target || primaryArg(c.name, c.input));
    const entry = groups.get(fam) ?? { count: 0, targets: [] };
    entry.count += 1;
    if (target && !entry.targets.includes(target) && entry.targets.length < 2) {
      entry.targets.push(target);
    }
    groups.set(fam, entry);
  }
  const verb: Record<string, string> = {
    read: 'Read',
    grep: 'Searched',
    find: 'Found',
    edit: 'Edited',
    write: 'Wrote',
    bash: 'Ran',
    websearch: 'Searched web',
    fetch: 'Fetched',
  };
  const bits: string[] = [];
  for (const [fam, info] of groups.entries()) {
    const v = verb[fam] || bareName(fam);
    if (info.count <= 2 && info.targets.length > 0) {
      bits.push(`${v} ${info.targets.join(', ')}`);
    } else if (info.targets.length > 0) {
      bits.push(`${v} ${info.count} (${info.targets[0]}…)`);
    } else {
      bits.push(`${v} ${info.count}`);
    }
    if (bits.length >= 3) break;
  }
  return bits.join(' · ');
}

function formatGroupTitle(children: any[] = [], counts: any = {}) {
  if (!Array.isArray(children) || children.length === 0) return formatGroupLabel(counts);
  const reads: string[] = [];
  const searches: string[] = [];
  const lists: string[] = [];
  for (const c of children) {
    if (c.verb === 'Thinking') continue;
    const t = shortTarget(c.target);
    if (c.kind === 'search') {
      if (t && !searches.includes(t)) searches.push(t);
    } else if (c.kind === 'list') {
      if (t && !lists.includes(t)) lists.push(t);
    } else if (t && !reads.includes(t)) {
      reads.push(t);
    }
  }
  const bits: string[] = [];
  if (counts.read > 0) {
    bits.push(
      counts.read <= 2 && reads.length > 0
        ? `Read ${reads.slice(0, 2).join(', ')}`
        : `Read ${counts.read} files${reads[0] ? ` (${reads[0]}…)` : ''}`,
    );
  }
  if (counts.search > 0) {
    bits.push(
      counts.search === 1 && searches[0]
        ? `Searched "${searches[0]}"`
        : `Searched ${counts.search} queries`,
    );
  }
  if (counts.list > 0) {
    bits.push(
      counts.list === 1 && lists[0] ? `Listed ${lists[0]}` : `Listed ${counts.list} dirs`,
    );
  }
  return bits.join(' · ') || formatGroupLabel(counts);
}

function createEditItem(run: any, meta: any) {
  // skill_manage has no display.diff; its ops hold the old and new text, so show those as the diff.
  const skill = bareName(run.name) === 'skill_manage' ? skillManageView(run.input) : null;
  const shown = skill ? { ...run, display: { ...run.display, diff: skill.diff } } : run;
  const stats = parseDiffStats(shown.display?.diff, skill ? skill.path : primaryArg(run.name, run.input));
  const intent = runTitle(run);
  return {
    type: 'edit',
    path: stats.path,
    title: intent || stats.path || 'Edited file',
    target: intent && stats.path && intent !== stats.path ? stats.path : '',
    additions: stats.additions,
    deletions: stats.deletions,
    ...meta,
    run: shown,
  };
}

function createExecuteItem(run: any, meta: any, exitCode: any, isFailed: any) {
  const code = executeCode(run.input);
  const command = primaryArg(run.name, run.input) || run.name;
  const errorTail = isFailed ? parseErrorTail(run.output) : '';
  const rawChildren = Array.isArray(run.children) ? run.children : [];
  const children = markChildOverlap(rawChildren);
  const summary = summarizeChildren(children);
  const mutationStats = childMutationStats(children);
  const intent = runTitle(run);
  const comment = executeHeaderComment(code);
  const childTitle = summarizeChildrenTitle(children);
  const title = intent || comment || childTitle || summarizeCodeCalls(code) || command;
  const target = (intent || comment) && summary ? summary : '';
  return {
    type: 'execute',
    command,
    title,
    target,
    code,
    summary,
    mutationStats,
    children,
    exitCode,
    errorTail,
    ...meta,
    run,
  };
}

function createWebItem(run: any, meta: any) {
  const arg = primaryArg(run.name, run.input) || run.name;
  const intent = runTitle(run);
  return { type: 'web', title: intent || arg, target: intent && intent !== arg ? arg : '', ...meta, run };
}

function createAgentItem(run: any, meta: any, fam: string) {
  const arg = primaryArg(run.name, run.input) || run.name;
  const intent = runTitle(run);
  const verb: Record<string, string> = {
    spawn: 'Spawned',
    steer: 'Steered',
    stop: 'Stopped',
    cancel: 'Cancelled',
    check: 'Checked',
    retry: 'Retried call',
  };
  const fallback = arg && arg !== run.name ? `${verb[fam] || 'Agent'} ${arg}` : verb[fam] || arg;
  return {
    type: 'agent',
    title: intent || fallback,
    target: intent && arg && intent !== arg ? arg : '',
    ...meta,
    run,
  };
}

function createSubmitItem(run: any, meta: any) {
  const json = parseJsonObject(run.input);
  const status = typeof json?.status === 'string' ? json.status.trim() : '';
  const summary = typeof json?.summary === 'string' ? json.summary.trim() : '';
  const intent = runTitle(run);
  const title =
    intent || (status === 'blocked' ? 'Submitted blocked report' : 'Submitted report');
  const target = summary && summary !== title ? summary : '';
  return { type: 'submit', title, target, ...meta, run };
}

function createBgItem(run: any, meta: any) {
  const arg = primaryArg(run.name, run.input) || run.name;
  const intent = runTitle(run);
  return { type: 'background', title: intent || arg, target: intent && intent !== arg ? arg : '', ...meta, run };
}

function createRanItem(run: any, meta: any, exitCode: any, isFailed: any) {
  const command = primaryArg(run.name, run.input) || run.name;
  const errorTail = isFailed ? parseErrorTail(run.output) : '';
  const intent = runTitle(run);
  return {
    type: 'ran',
    command,
    title: intent || command,
    target: intent && command && intent !== command ? command : '',
    exitCode,
    errorTail,
    ...meta,
    run,
  };
}

function createMcpItem(run: any, meta: any, mcp: any) {
  const target = primaryArg(run.name, run.input) || mcp.tool;
  const intent = runTitle(run);
  return {
    type: 'mcp',
    server: mcp.server,
    tool: mcp.tool,
    title: intent || `${mcp.server} / ${mcp.tool}`,
    target: target && target !== intent ? target : '',
    ...meta,
    run,
  };
}

const ITEM_CREATORS: Record<string, any> = {
  compress: (r: any, m: any) => {
    const target = primaryArg(r.name, r.input) || 'context';
    const intent = runTitle(r);
    return { type: 'compress', verb: 'Fold', title: intent || `Folded ${target}`, target: intent ? target : '', ...m, run: r };
  },
  decompress: (r: any, m: any) => {
    const target = primaryArg(r.name, r.input) || 'block';
    const intent = runTitle(r);
    return { type: 'decompress', verb: 'Restore', title: intent || `Restored ${target}`, target: intent ? target : '', ...m, run: r };
  },
  edit: (r: any, m: any) => createEditItem(r, m),
  write: (r: any, m: any) => createEditItem(r, m),
  websearch: createWebItem,
  fetch: createWebItem,
  spawn: (r: any, m: any) => createAgentItem(r, m, 'spawn'),
  steer: (r: any, m: any) => createAgentItem(r, m, 'steer'),
  stop: (r: any, m: any) => createAgentItem(r, m, 'stop'),
  cancel: (r: any, m: any) => createAgentItem(r, m, 'cancel'),
  check: (r: any, m: any) => createAgentItem(r, m, 'check'),
  retry: (r: any, m: any) => createAgentItem(r, m, 'retry'),
  submit: (r: any, m: any) => createSubmitItem(r, m),
  background: createBgItem,
  execute: createExecuteItem,
};

function createItemForRun(run: any, fam: any, meta: any, exitCode: any, isFailed: any) {
  const mcp = parseMcpName(run.name);
  if (mcp && mcp.server !== 'el') return createMcpItem(run, meta, mcp);
  const fn = ITEM_CREATORS[fam];
  return fn ? fn(run, meta, exitCode, isFailed) : createRanItem(run, meta, exitCode, isFailed);
}

function isThinking(run: any) {
  return run?.name === 'thinking' || Boolean(run?.thinking);
}

function thinkingText(run: any) {
  return run.output || run.text || run.title || '';
}

export function firstThinkingSentence(raw: string) {
  const lines = raw
    .split('\n')
    .map((l) => l.trim().replace(/^[#>*\-\s]+/, '').trim())
    .filter(Boolean);
  const first = lines[0] || 'Thinking';
  const sent = first.match(/^(.+?[.!?])(?:\s|$)/);
  const head = sent ? sent[1] : first;
  return head.length > 96 ? `${head.slice(0, 95)}…` : head;
}

function shouldJoinGroup(currentGroup: any, nextRun: any) {
  return Boolean(currentGroup && nextRun && isReadOnly(nextRun));
}

function checkThinkingRun(run: any, nextRun: any, currentGroup: any, items: any) {
  if (!isThinking(run)) return false;
  const raw = String(thinkingText(run) ?? '');
  const text = raw.trim();
  const isLive = Boolean(run.live || run.status === 'running');
  if (!text) {
    if (!isLive) return true;
    if (!shouldJoinGroup(currentGroup, nextRun)) {
      items.push({
        type: 'thinking',
        text: '',
        firstSentence: 'Thinking…',
        lineCount: 0,
        live: true,
        empty: true,
        duration: formatVisibleDuration(run.elapsedMs, true),
        run,
      });
    }
    return true;
  }
  if (shouldJoinGroup(currentGroup, nextRun)) {
    currentGroup.children.push({
      verb: 'Thinking',
      text: raw,
      firstSentence: firstThinkingSentence(text),
      lineCount: text.split('\n').length,
      live: isLive,
      duration: formatVisibleDuration(run.elapsedMs, isLive),
      run,
    });
  } else {
    const prev = items.at(-1);
    if (prev?.type === 'thinking' && !prev.empty) {
      const merged = `${prev.text.trimEnd()}\n\n${text}`;
      prev.text = merged;
      prev.lineCount = merged.split('\n').length;
      prev.live = prev.live || isLive;
      const totalMs = (Number(prev.run?.elapsedMs) || 0) + (Number(run.elapsedMs) || 0);
      prev.duration = formatVisibleDuration(totalMs, prev.live);
      return true;
    }
    items.push({
      type: 'thinking',
      text: raw,
      firstSentence: firstThinkingSentence(text),
      lineCount: text.split('\n').length,
      live: isLive,
      empty: false,
      duration: formatVisibleDuration(run.elapsedMs, isLive),
      run,
    });
  }
  return true;
}

function ensureGroup(currentGroup: any, items: any) {
  if (currentGroup) return currentGroup;
  const g = { type: 'group', counts: { list: 0, search: 0, read: 0 }, children: [], running: false, failed: false, runs: [] };
  items.push(g);
  return g;
}

function makeRunMeta(run: any, isRunning: any, isFailed: any) {
  const elapsedMs = Number(run.elapsedMs) || 0;
  const duration = formatVisibleDuration(elapsedMs, isRunning);
  return { running: isRunning, failed: isFailed, elapsedMs, duration };
}

// compactFeed turns a toolstrip block's runs into the compact feed model.
export function compactFeed(block: any, options: any = {}) {
  const runs = block?.runs ?? [];
  if (!Array.isArray(runs) || runs.length === 0) return [];
  if (options.collapsed ?? block?.collapsed ?? false) {
    return summarizeTurnLine(runs, block);
  }

  const items = [];
  let currentGroup: any = null;
  const closeGroup = () => {
    if (currentGroup) {
      currentGroup.label = formatGroupLabel(currentGroup.counts);
      currentGroup.title = formatGroupTitle(currentGroup.children, currentGroup.counts);
      currentGroup.elapsedMs = currentGroup.runs.reduce((sum: number, r: any) => sum + (Number(r.elapsedMs) || 0), 0);
      currentGroup.duration = formatVisibleDuration(currentGroup.elapsedMs, currentGroup.running);
      currentGroup = null;
    }
  };

  for (let i = 0; i < runs.length; i++) {
    const run = runs[i];
    if (checkThinkingRun(run, runs[i + 1], currentGroup, items)) {
      if (!currentGroup || !runs[i + 1] || !isReadOnly(runs[i + 1])) closeGroup();
      continue;
    }

    const isRunning = run.status === 'running';
    const isFailed = isRunFailed(run);

    if (isReadOnly(run)) {
      currentGroup = ensureGroup(currentGroup, items);
      appendToGroup(currentGroup, run, isRunning, isFailed);
      continue;
    }

    closeGroup();
    const meta = makeRunMeta(run, isRunning, isFailed);
    items.push(createItemForRun(run, family(run.name, run.input), meta, parseExitCode(run.output), isFailed));
  }

  closeGroup();
  return items;
}
// summarizeChildren formats child tools into a one-liner like "3 read · 1 edit · 1 bash".
export function summarizeChildren(children: any = []) {
  if (!Array.isArray(children) || children.length === 0) return '';
  const counts = new Map();
  for (const c of children) {
    let name = String(c.name || 'tool').toLowerCase();
    if (name.includes('__')) name = name.split('__').pop() ?? name;
    counts.set(name, (counts.get(name) || 0) + 1);
  }
  const bits = [];
  for (const [name, count] of counts.entries()) {
    bits.push(`${count} ${name}`);
  }
  return bits.join(' · ');
}

// childMutationStats finds diff stats for edit/write children.
export function childMutationStats(children: any = []) {
  if (!Array.isArray(children)) return [];
  const stats = [];
  for (const c of children) {
    const fam = family(c.name, c.input);
    if (fam === 'edit' || fam === 'write') {
      const parsed = parseDiffStats(c.display?.diff, primaryArg(c.name, c.input));
      if (parsed.path) {
        stats.push(parsed);
      }
    }
  }
  return stats;
}

// markChildOverlap tags children that ran concurrently.
export function markChildOverlap(children: any = []) {
  if (!Array.isArray(children) || children.length <= 1) return children;
  const ranges = children.map((c: any) => {
    const startSeq = c.events?.[0]?.seq ?? 0;
    const endSeq = c.status === 'running' ? Infinity : (c.events?.[c.events.length - 1]?.seq ?? startSeq);
    return { startSeq, endSeq };
  });

  return children.map((c: any, idx: any) => {
    const range = ranges[idx];
    const overlaps = ranges.some((other: any, oIdx: any) => {
      if (idx === oIdx) return false;
      return range.startSeq < other.endSeq && other.startSeq < range.endSeq;
    });
    return { ...c, overlap: overlaps };
  });
}
