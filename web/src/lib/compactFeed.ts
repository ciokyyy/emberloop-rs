// Compact feed model: turns a toolstrip block's runs into the compact structure.
// Pure model with no rendering and no side effects.
// Rules from .scratch/tool-feed-clarity/spec.md.

import { family, primaryArg, executeCode, parseMcpName } from './toolfeed';

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
export function isReadOnly(run: any) {
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

function createEditItem(run: any, meta: any) {
  const stats = parseDiffStats(run.display?.diff, primaryArg(run.name, run.input));
  return {
    type: 'edit',
    path: stats.path,
    additions: stats.additions,
    deletions: stats.deletions,
    ...meta,
    run,
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
  return {
    type: 'execute',
    command,
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
  return { type: 'web', title: primaryArg(run.name, run.input) || run.name, ...meta, run };
}

function createAgentItem(run: any, meta: any) {
  return { type: 'agent', title: primaryArg(run.name, run.input) || run.name, ...meta, run };
}

function createBgItem(run: any, meta: any) {
  return { type: 'background', title: primaryArg(run.name, run.input) || run.name, ...meta, run };
}

function createRanItem(run: any, meta: any, exitCode: any, isFailed: any) {
  const command = primaryArg(run.name, run.input) || run.name;
  const errorTail = isFailed ? parseErrorTail(run.output) : '';
  return { type: 'ran', command, exitCode, errorTail, ...meta, run };
}

function createMcpItem(run: any, meta: any, mcp: any) {
  return { type: 'mcp', server: mcp.server, tool: mcp.tool, target: primaryArg(run.name, run.input) || mcp.tool, ...meta, run };
}

const ITEM_CREATORS: Record<string, any> = {
  compress: (r: any, m: any) => ({ type: 'compress', verb: 'Fold', target: primaryArg(r.name, r.input) || 'context', ...m, run: r }),
  decompress: (r: any, m: any) => ({ type: 'decompress', verb: 'Restore', target: primaryArg(r.name, r.input) || 'block', ...m, run: r }),
  edit: (r: any, m: any) => createEditItem(r, m),
  write: (r: any, m: any) => createEditItem(r, m),
  websearch: createWebItem,
  fetch: createWebItem,
  spawn: createAgentItem,
  steer: createAgentItem,
  cancel: createAgentItem,
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

function shouldJoinGroup(currentGroup: any, nextRun: any) {
  return Boolean(currentGroup && nextRun && isReadOnly(nextRun));
}

function checkThinkingRun(run: any, nextRun: any, currentGroup: any, items: any) {
  if (!isThinking(run)) return false;
  const text = thinkingText(run);
  if (shouldJoinGroup(currentGroup, nextRun)) {
    currentGroup.children.push({ verb: 'Thinking', text, run });
  } else {
    items.push({ type: 'thinking', text, run });
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
  const duration = isRunning || elapsedMs >= 5000 ? formatDuration(elapsedMs) : null;
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
