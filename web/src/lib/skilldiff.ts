// skillDiffText renders the skill_manage ops a review or a turn applied as plain text: one header
// per op, then `-` lines for the removed text and `+` lines for the added text of a patch.
// Ops without old/new text (create, write_file, remove_file) show only their header.
export function skillDiffText(ops: unknown): string {
  if (!Array.isArray(ops)) return '';
  const out: string[] = [];
  for (const raw of ops) {
    const op = (raw ?? {}) as Record<string, unknown>;
    const name = typeof op.name === 'string' ? op.name : '?';
    const kind = typeof op.op === 'string' ? op.op : 'patch';
    const file = typeof op.path === 'string' ? ` ${op.path}` : '';
    out.push(`${name} · ${kind}${file}`);
    if (typeof op.old_string === 'string' && typeof op.new_string === 'string') {
      for (const line of op.old_string.split('\n')) out.push(`- ${line}`);
      for (const line of op.new_string.split('\n')) out.push(`+ ${line}`);
    }
  }
  return out.join('\n');
}

// skillManageView reads a skill_manage call's arguments: the skill names it touched and the diff of its ops.
export function skillManageView(input: unknown): { path: string; diff: string } {
  let ops: unknown = null;
  try {
    ops = JSON.parse(String(input ?? ''))?.ops;
  } catch {
    return { path: '', diff: '' };
  }
  const names = Array.isArray(ops) ? [...new Set(ops.map((op) => (op as { name?: unknown })?.name).filter((n) => typeof n === 'string'))] : [];
  return { path: names.join(', '), diff: skillDiffText(ops) };
}

/** lineDiff compares two texts line by line: `same`, `add` (only in `next`) or `del` (only in `prev`). */
export function lineDiff(prev: string, next: string): { kind: 'same' | 'add' | 'del'; text: string }[] {
  const a = prev.split('\n');
  const b = next.split('\n');
  // shortcut: O(lines x lines) table, fine for SKILL.md sizes, upgrade to Myers diff if skills grow to thousands of lines.
  const lcs = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const out: { kind: 'same' | 'add' | 'del'; text: string }[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ kind: 'same', text: a[i] });
      i++;
      j++;
    }
    else if (lcs[i + 1][j] >= lcs[i][j + 1]) out.push({ kind: 'del', text: a[i++] });
    else out.push({ kind: 'add', text: b[j++] });
  }
  while (i < a.length) out.push({ kind: 'del', text: a[i++] });
  while (j < b.length) out.push({ kind: 'add', text: b[j++] });
  return out;
}
