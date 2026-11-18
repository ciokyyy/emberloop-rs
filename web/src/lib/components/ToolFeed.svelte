<!-- Quiet dot tool feed: 32px rows (44px mobile), 5px status dot, intent title, always-visible duration, all rows closed by default except failed. -->
<script lang="ts">
  import { collapse } from '$lib/motion';
  import { compactFeed } from '$lib/compactFeed';
  import ToolBody from './ToolBody.svelte';
  import FileIcon from './FileIcon.svelte';
  import { bareName, primaryArg } from '$lib/toolfeed';
  import type { ToolRun } from '$lib/toolRuns';
  import { highlightShell } from '$lib/highlight';
  let { runs, options = {} }: { runs: ToolRun[]; options?: Record<string, unknown> } = $props();

  const items: any[] = $derived(compactFeed({ runs }, options));

  let expanded = $state<Record<string, boolean>>({});

  function isGroupExpanded(idx: number) {
    return expanded[`group-${idx}`] ?? false;
  }

  function toggleGroup(idx: number) {
    expanded[`group-${idx}`] = !isGroupExpanded(idx);
  }

  function isRowExpanded(key: string, defaultVal = false) {
    return expanded[key] ?? defaultVal;
  }

  function toggleRow(key: string, defaultVal = false) {
    expanded[key] = !isRowExpanded(key, defaultVal);
  }

  function childMs(ms: unknown) {
    const n = Number(ms) || 0;
    if (n <= 0) return '';
    return n < 1000 ? `${n}ms` : `${(n / 1000).toFixed(1)}s`;
  }
</script>

<div class="tool-feed font-sans text-[13px] leading-5 text-fg-2" data-testid="tool-feed" data-card="toolstrip">
  {#each items as item, idx (idx)}
    {#if item.type === 'turn-line'}
      {@render turnLine(item, idx)}
    {:else if item.type === 'group'}
      {@render exploredGroup(item, idx)}
    {:else if item.type === 'ran'}
      {@render ranRow(item, idx)}
    {:else if item.type === 'edit'}
      {@render editRow(item, idx)}
    {:else if item.type === 'thinking'}
      {@render thinkingRow(item, idx)}
    {:else if item.type === 'web'}
      {@render simpleRow(item, `web-${idx}`, 'web-row')}
    {:else if item.type === 'agent'}
      {@render simpleRow(item, `agent-${idx}`, 'agent-row')}
    {:else if item.type === 'submit'}
      {@render simpleRow(item, `submit-${idx}`, 'submit-row')}
    {:else if item.type === 'background'}
      {@render simpleRow(item, `bg-${idx}`, 'background-row')}
    {:else if item.type === 'execute'}
      {@render executeRow(item, idx)}
    {:else if item.type === 'compress'}
      {@render simpleRow(item, `compress-${idx}`, 'fold-row')}
    {:else if item.type === 'decompress'}
      {@render simpleRow(item, `decompress-${idx}`, 'restore-row')}
    {:else if item.type === 'mcp'}
      {@render simpleRow(item, `mcp-${idx}`, 'mcp-row', 'json')}
    {/if}
  {/each}
</div>

{#snippet dot(running: boolean, failed: boolean)}
  <span class="flex h-2 w-2 shrink-0 items-center justify-center" aria-hidden="true">
    <span
      class="h-[5px] w-[5px] rounded-full {failed
        ? 'bg-status-failed'
        : running
          ? 'running-dot bg-status-working'
          : 'bg-fg-4'}"
    ></span>
  </span>
{/snippet}

{#snippet chev(open: boolean)}
  <span
    class="shrink-0 text-[13px] leading-none text-fg-4 transition-transform duration-100 {open
      ? 'rotate-90 opacity-100'
      : 'opacity-40 group-hover/row:opacity-100'}"
    aria-hidden="true"
  >›</span>
{/snippet}

<!-- Turn collapse line -->
{#snippet turnLine(item: any, idx: number)}
  {@const open = isRowExpanded(`turn-${idx}`, false)}
  <button
    type="button"
    class="turn-line group/row flex min-h-[44px] w-full items-center gap-2.5 rounded-lg px-2 text-left select-none hover:bg-surface-2 focus-visible:ring-1 focus-visible:ring-border focus-visible:outline-none sm:min-h-[32px]"
    aria-expanded={open}
    onclick={() => toggleRow(`turn-${idx}`, false)}
  >
    {@render dot(false, item.failed)}
    {#if item.failed}<span class="sr-only">failed</span>{/if}
    <span class="min-w-0 flex-1 truncate text-[13px] text-fg-3">{item.label}</span>
    {#if item.totalTime}
      <span class="shrink-0 text-xs text-fg-4 tabular-nums">{item.totalTime}</span>
    {/if}
    {@render chev(open)}
  </button>

  {#if open}
    <div class="my-1 ml-[11px] space-y-1 border-l border-border pl-3">
      {#each compactFeed({ runs }, { ...options, collapsed: false }) as child, cIdx (cIdx)}
        <div class="py-0.5 text-xs text-fg-3">
          {child.title || child.label || child.command || child.path || child.firstSentence || ''}
        </div>
      {/each}
    </div>
  {/if}
{/snippet}

<!-- Explored Group -->
{#snippet exploredGroup(item: any, idx: number)}
  {@const open = isGroupExpanded(idx)}
  <div class="explored-group my-0.5">
    <button
      type="button"
      class="group-header group/row flex min-h-[44px] w-full items-center gap-2.5 rounded-lg px-2 text-left select-none hover:bg-surface-2 focus-visible:ring-1 focus-visible:ring-border focus-visible:outline-none sm:min-h-[32px] {item.running
        ? 'bg-surface-1'
        : ''}"
      aria-expanded={open}
      onclick={() => toggleGroup(idx)}
    >
      {@render dot(item.running, item.failed)}
      {#if item.running}<span class="sr-only">running</span>{/if}
      <span class="min-w-0 flex-1 truncate text-[13px] text-fg-3">{item.title || item.label}</span>
      <span class="sr-only">{item.label}</span>
      {#if item.duration}
        <span class="shrink-0 text-xs {item.running ? 'text-status-working' : 'text-fg-4'} tabular-nums">{item.duration}</span>
      {/if}
      {@render chev(open)}
    </button>

    {#if open}
      <div transition:collapse|global class="group-children relative my-1 ml-[11px] space-y-0.5 border-l border-border pl-3">
        {#each item.children as child, cIdx (cIdx)}
          {@render groupChild(child, idx, cIdx)}
        {/each}
      </div>
    {/if}
  </div>
{/snippet}

{#snippet groupChild(child: any, idx: number, cIdx: number)}
  {#if child.verb === 'Thinking'}
    {@render thinkingBlock(child, `group-${idx}-think-${cIdx}`, 'py-0.5')}
  {:else}
    {@const key = `group-${idx}-child-${cIdx}`}
    {@const open = isRowExpanded(key, false)}
    <div class="group-child">
      <button
        type="button"
        class="group/row flex min-h-[44px] w-full items-center gap-2 rounded-lg px-2 text-left hover:bg-surface-2 focus-visible:ring-1 focus-visible:ring-border focus-visible:outline-none sm:min-h-[28px]"
        aria-expanded={open}
        onclick={() => toggleRow(key, false)}
      >
        {#if child.kind === 'read'}
          <FileIcon path={child.target} />
        {:else}
          {@render dot(false, false)}
        {/if}
        <span class="w-[44px] shrink-0 text-xs text-fg-4">{child.verb}</span>
        <span class="min-w-0 flex-1 truncate font-mono text-[12px] text-fg-2">{child.target}</span>
        {@render chev(open)}
      </button>

      {#if open}
        <div transition:collapse|global class="expanded-box my-1 min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] rounded-lg border border-code-border bg-code-bg p-2.5 font-mono text-[12px] text-fg-1">
          <div class="mb-1 text-fg-3 select-text">$ {primaryArg(child.run?.name, child.run?.input) || bareName(child.run?.name)}</div>
          <ToolBody text={child.run?.output || '(no output)'} path={child.target} />
        </div>
      {/if}
    </div>
  {/if}
{/snippet}

{#snippet shellCommand(command: any)}{#each highlightShell(command) as tok, t (t)}{#if tok.cls}<span class={tok.cls}>{tok.text}</span>{:else}{tok.text}{/if}{/each}{/snippet}

<!-- Ran row -->
{#snippet ranRow(item: any, idx: number)}
  {#if item.failed}
    {@render ranFailed(item, idx)}
  {:else}
    {@const key = `ran-${idx}`}
    {@const open = isRowExpanded(key, false)}
    <div class="{item.running ? 'running-row' : 'ran-row'} my-0.5">
      <button
        type="button"
        class="group/row flex min-h-[44px] w-full items-center gap-2.5 rounded-lg px-2 text-left select-none hover:bg-surface-2 focus-visible:ring-1 focus-visible:ring-border focus-visible:outline-none sm:min-h-[32px] {item.running
          ? 'bg-surface-1'
          : ''}"
        aria-expanded={open}
        onclick={() => toggleRow(key, false)}
      >
        {@render dot(item.running, false)}
        {#if item.running}<span class="sr-only">running</span>{/if}
        <span class="min-w-0 truncate text-[13px] {item.target ? 'shrink-0 text-fg-2' : 'flex-1 font-mono text-[12.5px] text-fg-2'}">
          {item.title || item.command}
        </span>
        {#if item.target}
          <span class="min-w-0 flex-1 truncate font-mono text-[12px] text-fg-4">{item.target}</span>
        {/if}
        {#if item.duration}
          <span class="shrink-0 text-xs {item.running ? 'text-status-working' : 'text-fg-4'} tabular-nums">{item.duration}</span>
        {/if}
        {@render chev(open)}
      </button>

      {#if open}
        <div transition:collapse|global class="expanded-box my-1 ml-[11px] min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] rounded-lg border border-code-border bg-code-bg p-2.5 font-mono text-[12px] text-fg-1">
          <div class="mb-1 text-fg-3 select-text">$ {@render shellCommand(item.command)}</div>
          <ToolBody text={item.run?.output || '(no output)'} />
        </div>
      {/if}
    </div>
  {/if}
{/snippet}

<!-- Failed row with tint, open by default -->
{#snippet ranFailed(item: any, idx: number)}
  {@const key = `ran-${idx}`}
  {@const open = isRowExpanded(key, true)}
  <div class="failed-card my-1 min-w-0 max-w-full rounded-lg border border-transparent bg-[var(--tool-failed-tint)] p-2 hover:border-status-failed/20" data-testid="failed-row">
    <button
      type="button"
      class="group/row flex min-h-[44px] w-full items-center gap-2.5 text-left select-none focus-visible:outline-none sm:min-h-[28px]"
      aria-expanded={open}
      onclick={() => toggleRow(key, true)}
    >
      {@render dot(false, true)}
      <span class="sr-only">failed, exit {item.exitCode || 1}</span>
      <span class="min-w-0 flex-1 truncate font-mono text-[12.5px] text-fg-1">{item.title || item.command}</span>
      <span class="shrink-0 text-xs font-medium text-status-failed tabular-nums">
        {item.exitCode !== null ? `exit ${item.exitCode}` : 'failed'}{item.duration ? ` · ${item.duration}` : ''}
      </span>
      {@render chev(open)}
    </button>

    {#if item.errorTail}
      <div class="error-tail mt-1.5 ml-[18px] min-w-0 font-mono text-[12px] text-fg-3 whitespace-pre-wrap [overflow-wrap:anywhere] select-text">
        {item.errorTail}
      </div>
    {/if}

    {#if open}
      <div transition:collapse|global class="expanded-box mt-2 min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] rounded-lg border border-code-border bg-code-bg p-2.5 font-mono text-[12px] text-fg-1">
        <div class="mb-1 text-fg-3 select-text">$ {@render shellCommand(item.command)}</div>
        <ToolBody text={item.run?.output || '(no output)'} />
      </div>
    {/if}
  </div>
{/snippet}

<!-- Edit / Write row -->
{#snippet editRow(item: any, idx: number)}
  {@const key = `edit-${idx}`}
  {@const open = isRowExpanded(key, item.failed)}
  <div class="edit-row my-0.5 {item.failed ? 'rounded-lg bg-[var(--tool-failed-tint)] p-1.5' : ''}">
    <button
      type="button"
      class="group/row flex min-h-[44px] w-full items-center gap-2.5 rounded-lg px-2 text-left select-none hover:bg-surface-2 focus-visible:ring-1 focus-visible:ring-border focus-visible:outline-none sm:min-h-[32px] {item.running
        ? 'bg-surface-1'
        : ''}"
      aria-expanded={open}
      onclick={() => toggleRow(key, item.failed)}
    >
      {@render dot(item.running, item.failed)}
      <FileIcon path={item.path} />
      <span class="min-w-0 truncate text-[13px] {item.target ? 'shrink-0 text-fg-2' : 'flex-1 font-mono text-[12.5px] text-fg-2'}">
        {item.title || item.path}
      </span>
      {#if item.target}
        <span class="min-w-0 flex-1 truncate font-mono text-[12px] text-fg-4">{item.target}</span>
      {/if}
      {#if item.additions > 0}
        <span class="shrink-0 font-mono text-xs text-status-done tabular-nums">+{item.additions}</span>
      {/if}
      {#if item.deletions > 0}
        <span class="shrink-0 font-mono text-xs text-status-failed tabular-nums">−{item.deletions}</span>
      {/if}
      {#if item.duration}
        <span class="shrink-0 text-xs {item.running ? 'text-status-working' : 'text-fg-4'} tabular-nums">{item.duration}</span>
      {/if}
      {@render chev(open)}
    </button>

    {#if open}
      <div transition:collapse|global class="expanded-box my-1 ml-[11px] min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] rounded-lg border border-code-border bg-code-bg p-2.5 font-mono text-[12px] text-fg-1">
        <ToolBody text={item.run?.display?.diff || item.run?.output || '(no diff)'} path={item.path} language="diff" />
      </div>
    {/if}
  </div>
{/snippet}

<!-- Thinking row -->
{#snippet thinkingRow(item: any, idx: number)}
  {#if item.empty}
    <div class="thinking-pulse my-0.5 flex min-h-[44px] items-center gap-2.5 px-2 text-[13px] text-fg-4 sm:min-h-[32px]" data-testid="thinking-pulse">
      {@render dot(true, false)}
      <span>Thinking…</span>
      {#if item.duration}
        <span class="ml-auto shrink-0 text-xs text-status-working tabular-nums">{item.duration}</span>
      {/if}
    </div>
  {:else}
    {@render thinkingBlock(item, `think-${idx}`, 'my-0.5')}
  {/if}
{/snippet}

{#snippet thinkingBlock(item: any, key: string, cls: string)}
  {@const text = String(item.text ?? '').trim()}
  {@const lines = item.lineCount || (text ? text.split('\n').length : 0)}
  {@const header = item.firstSentence || text.split('\n')[0] || 'Thinking'}
  {@const open = isRowExpanded(key, false)}
  <div class="thinking-block {cls}">
    <button
      type="button"
      class="group/row flex min-h-[44px] w-full items-center gap-2.5 rounded-lg px-2 text-left select-none hover:bg-surface-2 focus-visible:ring-1 focus-visible:ring-border focus-visible:outline-none sm:min-h-[32px]"
      aria-expanded={open}
      onclick={() => toggleRow(key, false)}
    >
      {@render dot(Boolean(item.live), false)}
      <span class="min-w-0 flex-1 truncate text-[13px] text-fg-3">{header}</span>
      <span class="shrink-0 text-xs text-fg-4 tabular-nums">
        {#if item.duration}{item.duration} · {/if}{lines} {lines === 1 ? 'line' : 'lines'}
      </span>
      {@render chev(open)}
    </button>

    {#if open && text}
      <div
        transition:collapse|global
        class="my-1 ml-[11px] max-h-[240px] overflow-y-auto border-l border-border pl-3 pr-1 text-[13px] leading-5 text-fg-3 whitespace-pre-wrap [overflow-wrap:anywhere] select-text"
      >{text}</div>
    {/if}
  </div>
{/snippet}

<!-- Shared dot row for web, agent, submit, background, compress, decompress, mcp -->
{#snippet simpleRow(item: any, key: string, cls: string, language: string = '')}
  {@const open = isRowExpanded(key, item.failed)}
  <div class="{cls} my-0.5 {item.failed ? 'rounded-lg bg-[var(--tool-failed-tint)] p-1.5' : ''}">
    <button
      type="button"
      class="group/row flex min-h-[44px] w-full items-center gap-2.5 rounded-lg px-2 text-left select-none hover:bg-surface-2 focus-visible:ring-1 focus-visible:ring-border focus-visible:outline-none sm:min-h-[32px] {item.running
        ? 'bg-surface-1'
        : ''}"
      aria-expanded={open}
      onclick={() => toggleRow(key, item.failed)}
    >
      {@render dot(item.running, item.failed)}
      <span class="min-w-0 truncate text-[13px] {item.target ? 'shrink-0 text-fg-2' : 'flex-1 text-fg-2'}">
        {item.title}
      </span>
      {#if item.target}
        <span class="min-w-0 flex-1 truncate font-mono text-[12px] text-fg-4">{item.target}</span>
      {/if}
      {#if item.duration}
        <span class="shrink-0 text-xs {item.running ? 'text-status-working' : 'text-fg-4'} tabular-nums">{item.duration}</span>
      {/if}
      {@render chev(open)}
    </button>

    {#if open}
      <div transition:collapse|global class="expanded-box my-1 ml-[11px] min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] rounded-lg border border-code-border bg-code-bg p-2.5 font-mono text-[12px] text-fg-1">
        <ToolBody text={item.run?.output || '(no output)'} {language} />
      </div>
    {/if}
  </div>
{/snippet}

<!-- Execute row: always closed unless failed -->
{#snippet executeRow(item: any, idx: number)}
  {@const key = `exec-${idx}`}
  {@const open = isRowExpanded(key, item.failed)}
  <div
    class="execute-row my-0.5 {item.failed
      ? 'failed-card rounded-lg border border-transparent bg-[var(--tool-failed-tint)] p-2'
      : ''}"
    data-testid="toolfeed-execute-row"
  >
    <button
      type="button"
      class="group/row flex min-h-[44px] w-full items-center gap-2.5 rounded-lg px-2 text-left select-none hover:bg-surface-2 focus-visible:ring-1 focus-visible:ring-border focus-visible:outline-none sm:min-h-[32px] {item.running
        ? 'bg-surface-1'
        : ''}"
      aria-expanded={open}
      onclick={() => toggleRow(key, item.failed)}
    >
      {@render dot(item.running, item.failed)}
      {#if item.running}<span class="sr-only">running</span>{/if}
      <span class="min-w-0 truncate text-[13px] text-fg-2 {item.target ? 'shrink-0' : 'flex-1'}">
        {item.title || item.summary || item.command}
      </span>
      {#if item.target}
        <span class="min-w-0 flex-1 truncate font-mono text-[12px] text-fg-4">{item.target}</span>
      {/if}
      {#if item.mutationStats && item.mutationStats.length > 0}
        <span class="flex shrink-0 items-center gap-1.5 font-mono text-xs">
          {#each item.mutationStats as stat, sIdx (sIdx)}
            <span class="max-w-[140px] truncate text-fg-3">{stat.path}</span>
            <span class="text-status-done">+{stat.additions}</span>
            <span class="text-status-failed">−{stat.deletions}</span>
          {/each}
        </span>
      {/if}
      {#if item.duration}
        <span class="shrink-0 text-xs {item.running ? 'text-status-working' : item.failed ? 'text-status-failed' : 'text-fg-4'} tabular-nums">{item.duration}</span>
      {/if}
      {@render chev(open)}
    </button>

    {#if item.failed && item.errorTail}
      <div class="error-tail mt-1.5 ml-[18px] min-w-0 font-mono text-[12px] text-fg-3 whitespace-pre-wrap [overflow-wrap:anywhere] select-text">
        {item.errorTail}
      </div>
    {/if}

    {#if open}
      {@render executeExpanded(item)}
    {/if}
  </div>
{/snippet}

{#snippet executeExpanded(item: any)}
  <div transition:collapse|global class="expanded-box my-1 ml-[11px] min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] rounded-lg border border-code-border bg-code-bg p-2.5 font-mono text-[12px] text-fg-1">
    {#if item.children && item.children.length > 0}
      <div class="mb-2.5 space-y-1">
        <div class="text-[11px] font-medium text-fg-3">Calls</div>
        {#each item.children as child, cIdx (child.toolId || child.key || cIdx)}
          {@render execChild(child)}
        {/each}
      </div>
    {/if}

    {#if item.code}
      <div class="mt-2">
        <div class="mb-1 text-[11px] font-medium text-fg-3">Code</div>
        <ToolBody text={item.code} language="code" />
      </div>
    {:else if item.run?.input}
      <pre class="mb-2 whitespace-pre-wrap text-fg-3">{item.run.input}</pre>
    {/if}

    {#if item.run?.output}
      <div class="mt-2 border-t border-border pt-2">
        <div class="mb-1 text-[11px] font-medium text-fg-3">Output</div>
        <ToolBody text={item.run.output} />
      </div>
    {/if}
  </div>
{/snippet}

{#snippet execChild(child: any)}
  <div class="child-row border-l border-border py-0.5 pl-2 text-xs">
    <div class="flex items-center gap-2 font-mono text-fg-2">
      {@render dot(child.status === 'running', child.status === 'failed')}
      <span class="shrink-0 text-fg-3">{bareName(child.name)}</span>
      {#if child.overlap}
        <span class="rounded bg-surface-3 px-1 text-[10px] text-fg-3">parallel</span>
      {/if}
      <span class="min-w-0 flex-1 truncate text-fg-4">{child.target || child.input}</span>
      {#if child.status === 'running'}
        <span class="text-[11px] text-status-working">running…</span>
      {:else if child.status === 'failed'}
        <span class="text-[11px] text-status-failed">failed</span>
      {:else if childMs(child.elapsedMs)}
        <span class="shrink-0 text-[11px] text-fg-4 tabular-nums">{childMs(child.elapsedMs)}</span>
      {/if}
    </div>
    {#if child.display?.diff}
      <div class="mt-1 rounded bg-[#111] p-1.5 font-mono text-[11px] whitespace-pre-wrap">
        <ToolBody text={child.display.diff} />
      </div>
    {:else if child.output}
      <div class="mt-1 max-h-40 overflow-auto rounded bg-[#111] p-1.5 font-mono text-[11px] text-fg-3 whitespace-pre-wrap">
        <ToolBody text={child.output} />
      </div>
    {/if}
  </div>
{/snippet}

<style>
  .tool-feed {
    padding: 2px 0;
  }

  /* The embed icon carries its own right margin for inline text; a flex row already has a gap. */
  .tool-feed :global(.md-file-icon) {
    margin-right: 0;
  }

  .running-dot {
    animation: pulse 900ms infinite ease-in-out;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; transform: scale(1); }
    50% { opacity: 0.4; transform: scale(0.85); }
  }

  @media (prefers-reduced-motion: reduce) {
    .running-dot {
      animation: none;
    }
    .group-children, .expanded-box {
      transition: none !important;
    }
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border-width: 0;
  }
</style>
