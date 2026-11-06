import { describe, it, expect } from 'vitest';
import {
  compactFeed,
  parseExitCode,
  parseErrorTail,
  parseDiffStats,
  formatDuration,
  formatTotalTime,
  formatGroupLabel,
  summarizeChildren,
  childMutationStats,
  markChildOverlap,
} from './compactFeed';

describe('compactFeed helper parsers', () => {
  it('parses exit codes from bash output', () => {
    expect(parseExitCode('some output\n[exit status 1]')).toBe(1);
    expect(parseExitCode('error line\n[exit status 127]\n')).toBe(127);
    expect(parseExitCode('all clean')).toBe(null);
  });

  it('extracts two-line error tails', () => {
    const out = 'error: file not found\nfailed to execute\n[exit status 1]';
    expect(parseErrorTail(out)).toBe('error: file not found\nfailed to execute');

    const single = 'fatal: bad revision\n[exit status 128]';
    expect(parseErrorTail(single)).toBe('fatal: bad revision');

    expect(parseErrorTail('[exit status 2]')).toBe('');
  });

  it('parses diff stats', () => {
    const diff = `--- a/src/app.js
+++ b/src/app.js
@@ -1,3 +1,4 @@
 const a = 1;
-const b = 2;
+const b = 3;
+const c = 4;
`;
    const stats = parseDiffStats(diff);
    expect(stats.path).toBe('src/app.js');
    expect(stats.additions).toBe(2);
    expect(stats.deletions).toBe(1);
  });

  it('formats duration and total time', () => {
    expect(formatDuration(5000)).toBe('5s');
    expect(formatDuration(12000)).toBe('12s');
    expect(formatTotalTime(242000)).toBe('4m 02s');
    expect(formatTotalTime(12000)).toBe('12s');
  });

  it('formats group labels correctly', () => {
    expect(formatGroupLabel({ list: 2, search: 1, read: 1 })).toBe(
      'Explored 2 listings · 1 search · 1 file'
    );
    expect(formatGroupLabel({ read: 3 })).toBe('Explored 3 files');
    expect(formatGroupLabel({ list: 1 })).toBe('Explored 1 listing');
  });
});

describe('compactFeed grouping and breakout', () => {
  it('groups consecutive read-only runs and counts match child rows', () => {
    const block = {
      runs: [
        {
          name: 'bash',
          input: '{"command":"ls -la"}',
          output: 'total 0',
          status: 'completed',
          display: {
            kind: 'list',
            parts: JSON.stringify([{ kind: 'list', target: '.' }]),
          },
        },
        {
          name: 'bash',
          input: '{"command":"rg TODO src"}',
          output: 'src/main.go: TODO',
          status: 'completed',
          display: {
            kind: 'search',
            parts: JSON.stringify([{ kind: 'search', target: '"TODO" in src' }]),
          },
        },
        {
          name: 'read',
          input: '{"path":"src/main.go"}',
          output: 'package main',
          status: 'completed',
        },
      ],
    };

    const items = compactFeed(block);
    expect(items.length).toBe(1);
    const group = items[0];
    expect(group.type).toBe('group');
    expect(group.counts).toEqual({ list: 1, search: 1, read: 1 });
    expect(group.label).toBe('Explored 1 listing · 1 search · 1 file');
    expect(group.children.length).toBe(3);
    expect(group.children[0]).toMatchObject({ verb: 'List', target: '.' });
    expect(group.children[1]).toMatchObject({ verb: 'Search', target: '"TODO" in src' });
    expect(group.children[2]).toMatchObject({ verb: 'Read', target: 'src/main.go' });
  });

  it('breaks out a failed call and reports it in collapsed turn', () => {
    const block = {
      runs: [
        {
          name: 'read',
          input: '{"path":"src/main.go"}',
          output: 'package main',
          status: 'completed',
        },
        {
          name: 'bash',
          input: '{"command":"cat missing.txt"}',
          output: 'cat: missing.txt: No such file or directory\n[exit status 1]',
          status: 'failed',
          display: { kind: 'read' },
        },
        {
          name: 'read',
          input: '{"path":"src/other.go"}',
          output: 'package other',
          status: 'completed',
        },
      ],
    };

    const items = compactFeed(block);
    expect(items.length).toBe(3);
    expect(items[0].type).toBe('group');
    expect(items[0].children.length).toBe(1);

    expect(items[1].type).toBe('ran');
    expect(items[1].failed).toBe(true);
    expect(items[1].exitCode).toBe(1);
    expect(items[1].errorTail).toBe('cat: missing.txt: No such file or directory');

    expect(items[2].type).toBe('group');
    expect(items[2].children.length).toBe(1);

    // Turn boundary collapse: reports failure
    const collapsed = compactFeed(block, { collapsed: true });
    expect(collapsed.length).toBe(1);
    expect(collapsed[0].type).toBe('turn-line');
    expect(collapsed[0].failed).toBe(true);
    expect(collapsed[0].counts.explored).toBe(2);
    expect(collapsed[0].counts.ran).toBe(1);
  });

  it('thinking inside a burst joins the group; thinking after an edit does not', () => {
    const block = {
      runs: [
        {
          name: 'read',
          input: '{"path":"src/a.go"}',
          status: 'completed',
        },
        {
          name: 'thinking',
          output: 'analyzing dependencies...',
          thinking: true,
        },
        {
          name: 'read',
          input: '{"path":"src/b.go"}',
          status: 'completed',
        },
        {
          name: 'edit',
          input: '{"path":"src/b.go"}',
          display: { diff: '--- a/src/b.go\n+++ b/src/b.go\n+new line\n' },
          status: 'completed',
        },
        {
          name: 'thinking',
          output: 'now checking results...',
          thinking: true,
        },
      ],
    };

    const items = compactFeed(block);
    expect(items.length).toBe(3);

    // Item 0: group holding read a.go, thinking, read b.go
    expect(items[0].type).toBe('group');
    expect(items[0].children.length).toBe(3);
    expect(items[0].children[0].verb).toBe('Read');
    expect(items[0].children[1].verb).toBe('Thinking');
    expect(items[0].children[2].verb).toBe('Read');

    // Item 1: edit row
    expect(items[1].type).toBe('edit');
    expect(items[1].path).toBe('src/b.go');
    expect(items[1].additions).toBe(1);

    // Item 2: standalone thinking row after edit
    expect(items[2].type).toBe('thinking');
    expect(items[2].text).toBe('now checking results...');
  });

  it('renders bash run with no display.kind as ran, never as exploring', () => {
    const block = {
      runs: [
        {
          name: 'bash',
          input: '{"command":"go build ./..."}',
          status: 'completed',
          elapsedMs: 6200,
        },
      ],
    };

    const items = compactFeed(block);
    expect(items.length).toBe(1);
    expect(items[0].type).toBe('ran');
    expect(items[0].command).toBe('go build ./...');
    expect(items[0].duration).toBe('6s');
  });

  it('handles old logs and logs with kill switch off gracefully', () => {
    const block = {
      runs: [
        {
          name: 'bash',
          input: '{"command":"ls -la"}',
          status: 'completed',
          // no display object (kill switch or old log)
        },
        {
          name: 'read',
          input: '{"path":"file.txt"}',
          status: 'completed',
        },
      ],
    };

    const items = compactFeed(block);
    expect(items.length).toBe(2);
    expect(items[0].type).toBe('ran');
    expect(items[1].type).toBe('group');
    expect(items[1].children.length).toBe(1);
  });

  it('covers table tests: empty block, running tail, interleaved agents', () => {
    // Empty block
    expect(compactFeed(null)).toEqual([]);
    expect(compactFeed({ runs: [] })).toEqual([]);

    // Running tail
    const runningBlock = {
      runs: [
        {
          name: 'read',
          input: '{"path":"a.go"}',
          status: 'completed',
        },
        {
          name: 'bash',
          input: '{"command":"grep TODO b.go"}',
          status: 'running',
          elapsedMs: 200,
          display: { kind: 'search' },
        },
      ],
    };
    const runningItems = compactFeed(runningBlock);
    expect(runningItems.length).toBe(1);
    expect(runningItems[0].type).toBe('group');
    expect(runningItems[0].running).toBe(true);

    // Interleaved agent
    const agentBlock = {
      runs: [
        {
          name: 'read',
          input: '{"path":"a.go"}',
          status: 'completed',
        },
        {
          name: 'agents',
          input: '{"action":"spawn","name":"worker"}',
          status: 'completed',
        },
        {
          name: 'read',
          input: '{"path":"b.go"}',
          status: 'completed',
        },
      ],
    };
    const agentItems = compactFeed(agentBlock);
    expect(agentItems.length).toBe(3);
    expect(agentItems[0].type).toBe('group');
    expect(agentItems[1].type).toBe('agent');
    expect(agentItems[2].type).toBe('group');
  });

  it('produces execute item with extracted code when execute runs', () => {
    const block = {
      runs: [
        {
          name: 'execute',
          input: JSON.stringify({ code: 'import sys\nprint("hi")' }),
          output: 'hi\n',
          status: 'completed',
        },
        {
          name: 'execute',
          input: 'invalid json',
          output: 'syntax error',
          status: 'failed',
        },
      ],
    };
    const items = compactFeed(block);
    expect(items.length).toBe(2);
    expect(items[0].type).toBe('execute');
    expect(items[0].code).toBe('import sys\nprint("hi")');
    expect(items[1].type).toBe('execute');
    expect(items[1].code).toBe('');
    expect(items[1].failed).toBe(true);
  });
});

describe('Ticket 21 inner tool execute display', () => {
  it('summarizes child tools in order', () => {
    const children = [
      { name: 'read' },
      { name: 'read' },
      { name: 'read' },
      { name: 'edit' },
      { name: 'bash' },
    ];
    expect(summarizeChildren(children)).toBe('3 read · 1 edit · 1 bash');
  });

  it('extracts mutation diff stats for edit/write children', () => {
    const diff = `--- a/main.go\n+++ b/main.go\n@@ -1,2 +1,3 @@\n+line1\n+line2\n-old\n`;
    const children = [
      { name: 'read', input: '{"path":"main.go"}' },
      { name: 'edit', input: '{"path":"main.go"}', display: { diff } },
    ];
    const stats = childMutationStats(children);
    expect(stats.length).toBe(1);
    expect(stats[0].path).toBe('main.go');
    expect(stats[0].additions).toBe(2);
    expect(stats[0].deletions).toBe(1);
  });

  it('marks overlapping concurrent children', () => {
    const children = [
      {
        name: 'read',
        events: [{ seq: 1 }, { seq: 5 }],
        status: 'completed',
      },
      {
        name: 'grep',
        events: [{ seq: 2 }, { seq: 4 }],
        status: 'completed',
      },
      {
        name: 'read',
        events: [{ seq: 10 }, { seq: 12 }],
        status: 'completed',
      },
    ];
    const marked = markChildOverlap(children);
    expect(marked[0].overlap).toBe(true);
    expect(marked[1].overlap).toBe(true);
    expect(marked[2].overlap).toBe(false);
  });

  it('folds an execute made only of read-only children into the Explored group', () => {
    const block = {
      runs: [
        {
          name: 'execute',
          input: '{"code":"import tools\\n..."}',
          status: 'completed',
          children: [
            { name: 'read', input: '{"path":"foo.go"}', status: 'completed' },
            { name: 'grep', input: '{"pattern":"bar"}', status: 'completed' },
          ],
        },
      ],
    };
    const items = compactFeed(block);
    expect(items.length).toBe(1);
    expect(items[0].type).toBe('group');
    expect(items[0].counts.read).toBe(1);
    expect(items[0].counts.search).toBe(1);
    expect(items[0].label).toBe('Explored 1 search · 1 file');
  });

  it('keeps execute item when it has mixed or mutating children, showing mutation stats on collapsed line', () => {
    const diff = `--- a/foo.go\n+++ b/foo.go\n@@ -1 +1,2 @@\n+new line\n`;
    const block = {
      runs: [
        {
          name: 'execute',
          input: '{"code":"import tools\\n..."}',
          status: 'completed',
          children: [
            { name: 'read', input: '{"path":"foo.go"}', status: 'completed' },
            { name: 'edit', input: '{"path":"foo.go"}', display: { diff }, status: 'completed' },
          ],
        },
      ],
    };
    const items = compactFeed(block);
    expect(items.length).toBe(1);
    expect(items[0].type).toBe('execute');
    expect(items[0].summary).toBe('1 read · 1 edit');
    expect(items[0].mutationStats.length).toBe(1);
    expect(items[0].mutationStats[0].path).toBe('foo.go');
    expect(items[0].mutationStats[0].additions).toBe(1);
  });

  it('keeps legacy execute rows with no children unchanged', () => {
    const block = {
      runs: [
        {
          name: 'execute',
          input: '{"code":"print(1)"}',
          output: '1\n',
          status: 'completed',
        },
      ],
    };
    const items = compactFeed(block);
    expect(items.length).toBe(1);
    expect(items[0].type).toBe('execute');
    expect(items[0].children).toEqual([]);
    expect(items[0].summary).toBe('');
  });
});

describe('el bash exit header', () => {
  it('reads the exit code from the first line', () => {
    expect(parseExitCode('exit code: 2\nboom')).toBe(2);
    expect(parseExitCode('exit code: 0\nok')).toBe(0);
    expect(parseErrorTail('exit code: 2\nline a\nline b')).toBe('line a\nline b');
  });
});
