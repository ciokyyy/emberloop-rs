<!-- Compact tool feed: calm, folded read-only exploration with quiet finished rows.
  Rules from .scratch/tool-feed-clarity/spec.md and design.md. -->
<script lang="ts">
  import Icon, { type IconName } from './Icon.svelte';
  import { collapse } from '$lib/motion';
  import { compactFeed } from '$lib/compactFeed';
  import ToolBody from './ToolBody.svelte';
  import { bareName, primaryArg } from '$lib/toolfeed';
  import type { ToolRun } from '$lib/toolRuns';
  import { highlightShell } from '$lib/highlight';
  let { runs, options = {} }: { runs: ToolRun[]; options?: Record<string, unknown> } = $props();

  // Model items generated from compactFeed pure function.
  const items: any[] = $derived(compactFeed({ runs }, options));

  // Expansion state per item key.
  let expanded = $state<Record<string, boolean>>({});

  function isPhone() {
    return typeof window !== 'undefined' && window.innerWidth <= 640;
  }

  // Groups default collapsed on phone, expanded on desktop.
  function isGroupExpanded(idx: number) {
    if (expanded[`group-${idx}`] !== undefined) {
      return expanded[`group-${idx}`];
    }
    return !isPhone();
  }

  function toggleGroup(idx: number) {
    expanded[`group-${idx}`] = !isGroupExpanded(idx);
  }

  // The last tool row is open by default; it closes when the next item arrives
  // unless the User opened or closed it explicitly.
  const LAST_OPEN_ROW = /^(?:ran|edit|web|compress|decompress|mcp)-(\d+)$/;
  function isRowExpanded(key: string, defaultVal = false) {
    if (expanded[key] !== undefined) return expanded[key];
    const m = LAST_OPEN_ROW.exec(key);
    return defaultVal || (m !== null && Number(m[1]) === items.length - 1);
  }

  function toggleRow(key: string, defaultVal = false) {
    expanded[key] = !isRowExpanded(key, defaultVal);
  }
</script>

<div class="tool-feed font-sans text-xs leading-5 text-[var(--fg-2)]" data-testid="tool-feed" data-card="toolstrip">
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
      {@render webRow(item, idx)}
    {:else if item.type === 'agent'}
      {@render agentRow(item, idx)}
    {:else if item.type === 'background'}
      {@render backgroundRow(item, idx)}
    {:else if item.type === 'execute'}
      {@render executeRow(item, idx)}
    {:else if item.type === 'compress'}
      {@render foldRow(item, idx)}
    {:else if item.type === 'decompress'}
      {@render restoreRow(item, idx)}
    {:else if item.type === 'mcp'}
      {@render mcpRow(item, idx)}
    {/if}
  {/each}
</div>

<!-- Turn collapse line -->
{#snippet turnLine(item: any, idx: number)}
  <button
    type="button"
    class="turn-line flex w-full items-center gap-2 rounded px-2 py-1 text-left select-none hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[28px] sm:min-h-[28px] min-h-[44px]"
    onclick={() => toggleRow(`turn-${idx}`, false)}
  >
    <span class="text-[var(--fg-4)] shrink-0">
      {#if isRowExpanded(`turn-${idx}`, false)}
        <Icon name="chevron-down" size={14} />
      {:else}
        <Icon name="chevron-right" size={14} />
      {/if}
    </span>
    {#if item.failed}
      <span class="h-2 w-2 rounded-full bg-[var(--status-failed)] shrink-0" aria-hidden="true"></span>
      <span class="sr-only">failed</span>
    {/if}
    <span class="text-[13px] font-medium text-[var(--fg-3)] truncate flex-1">{item.label}</span>
    <span class="text-xs text-[var(--fg-4)] tabular-nums shrink-0">{item.totalTime}</span>
  </button>

  {#if isRowExpanded(`turn-${idx}`, false)}
    <div class="pl-4 border-l border-[var(--border)] my-1 space-y-1">
      <!-- Render expanded turn items if uncollapsed on click -->
      {#each compactFeed({ runs }, { ...options, collapsed: false }) as child, cIdx (cIdx)}
        <div class="py-0.5 text-xs text-[var(--fg-3)]">
          {child.type}: {child.label || child.command || child.path || child.text || ''}
        </div>
      {/each}
    </div>
  {/if}
{/snippet}

<!-- Explored Group -->
{#snippet exploredGroup(item: any, idx: number)}
  <div class="explored-group my-0.5">
    <button
      type="button"
      class="group-header flex w-full items-center gap-2 rounded px-2 py-1 text-left select-none hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[28px] sm:min-h-[28px] min-h-[44px]"
      onclick={() => toggleGroup(idx)}
    >
      <span class="text-[var(--fg-4)] shrink-0">
        {#if isGroupExpanded(idx)}
          <Icon name="chevron-down" size={14} />
        {:else}
          <Icon name="chevron-right" size={14} />
        {/if}
      </span>
      {#if item.running}
        <span class="running-dot h-2 w-2 rounded-full bg-[var(--status-working)] shrink-0" aria-hidden="true"></span>
        <span class="sr-only">running</span>
      {/if}
      <span class="text-[13px] font-medium text-[var(--fg-2)] shrink-0">Explored</span>
      <span class="sr-only">{item.label}</span>
      <div class="flex items-center gap-2.5 text-xs text-[var(--fg-3)] truncate flex-1">
        {@render groupCount(item.counts?.read, 'file-text', 'text-[var(--tool-read)]', 'file', 'files')}
        {@render groupCount(item.counts?.search, 'search', 'text-[var(--tool-grep)]', 'search', 'searches')}
        {@render groupCount(item.counts?.list, 'list', 'text-[var(--tool-web)]', 'listing', 'listings')}
      </div>
      {#if item.duration}
        <span class="text-xs text-[var(--fg-4)] tabular-nums shrink-0">{item.duration}</span>
      {/if}
    </button>

    {#if isGroupExpanded(idx)}
      <div transition:collapse|global class="group-children relative ml-3 pl-3 border-l border-[var(--border)] my-1 space-y-0.5">
        {#each item.children as child, cIdx (cIdx)}
          {@render groupChild(child, idx, cIdx)}
        {/each}
      </div>
    {/if}
  </div>
{/snippet}

{#snippet groupChild(child: any, idx: number, cIdx: number)}
  {#if child.verb === 'Thinking'}
    {@render thinkingBlock(child.text, `group-${idx}-think-${cIdx}`, 'pl-2.5 py-0.5', 'text-[12px]')}
  {:else}
    <div class="group-child">
      <button
        type="button"
        class="flex w-full items-center gap-2.5 rounded px-2 py-0.5 text-left hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[24px] sm:min-h-[24px] min-h-[44px]"
        onclick={() => toggleRow(`group-${idx}-child-${cIdx}`, false)}
      >
        {#if child.kind === 'read'}
          <Icon name="file-text" size={13} class="text-[var(--tool-read)] shrink-0" />
        {:else if child.kind === 'search'}
          <Icon name="search" size={13} class="text-[var(--tool-grep)] shrink-0" />
        {:else if child.kind === 'list'}
          <Icon name="list" size={13} class="text-[var(--tool-web)] shrink-0" />
        {/if}
        <span class="w-[42px] shrink-0 text-xs font-medium text-[var(--fg-4)]">{child.verb}</span>
        <span class="font-mono text-[12.5px] text-[var(--fg-2)] truncate flex-1">{child.target}</span>
      </button>

      {#if isRowExpanded(`group-${idx}-child-${cIdx}`, false)}
        <div transition:collapse|global class="expanded-box min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] bg-[#1c1c1e] border border-[#2c2c2e] rounded-lg p-2.5 my-1 font-mono text-[11.5px] text-[var(--fg-1)]">
          <div class="text-[var(--fg-3)] mb-1 select-text">$ {primaryArg(child.run?.name, child.run?.input) || bareName(child.run?.name)}</div>
          <ToolBody text={child.run?.output || '(no output)'} path={child.target} />
        </div>
      {/if}
    </div>
  {/if}
{/snippet}

{#snippet shellCommand(command: any)}{#each highlightShell(command) as tok, t (t)}{#if tok.cls}<span class={tok.cls}>{tok.text}</span>{:else}{tok.text}{/if}{/each}{/snippet}

<!-- Ran row -->
{#snippet ranRow(item: any, idx: number)}
  {#if item.running}
    {@render ranRunning(item)}
  {:else if item.failed}
    {@render ranFailed(item, idx)}
  {:else}
    {@render ranDone(item, idx)}
  {/if}
{/snippet}

<!-- Running row -->
{#snippet ranRunning(item: any)}
  <div class="running-row flex w-full items-center gap-2 rounded-lg bg-[var(--surface-2)] px-2 py-1.5 my-0.5 min-h-[44px] sm:min-h-[32px]">
    <span class="running-dot h-[7px] w-[7px] rounded-full bg-[var(--status-working)] shrink-0" aria-hidden="true"></span>
    <span class="sr-only">running</span>
    <span class="font-medium text-[var(--fg-1)] shrink-0">Running…</span>
    <span class="font-mono text-[12.5px] text-[var(--fg-3)] truncate flex-1">{item.command}</span>
    <span class="text-xs text-[var(--status-working)] tabular-nums shrink-0">{item.duration || '0s'}</span>
  </div>
{/snippet}

<!-- Failed row with tint -->
{#snippet ranFailed(item: any, idx: number)}
  <div class="failed-card min-w-0 max-w-full rounded-lg bg-[var(--tool-failed-tint)] p-2 my-1 border border-transparent hover:border-[var(--status-failed)]/20" data-testid="failed-row">
    <button
      type="button"
      class="flex w-full items-center gap-2 text-left select-none focus-visible:outline-none min-h-[44px] sm:min-h-[28px]"
      onclick={() => toggleRow(`ran-${idx}`, true)}
    >
      <span class="h-[7px] w-[7px] rounded-full bg-[var(--status-failed)] shrink-0" aria-hidden="true"></span>
      <span class="sr-only">failed, exit {item.exitCode || 1}</span>
      <span class="font-medium text-[var(--fg-3)] shrink-0">Ran</span>
      <span class="font-mono text-[12.5px] text-[var(--fg-1)] truncate flex-1">{item.command}</span>
      <span class="font-medium text-[var(--status-failed)] shrink-0">{item.exitCode !== null ? `exit ${item.exitCode}` : 'failed'}{item.duration ? ` · ${item.duration}` : ''}</span>
    </button>

    {#if item.errorTail}
      <div class="error-tail mt-1.5 ml-4 min-w-0 font-mono text-[11.5px] text-[var(--fg-3)] whitespace-pre-wrap [overflow-wrap:anywhere] select-text">
        {item.errorTail}
      </div>
    {/if}

    {#if isRowExpanded(`ran-${idx}`, true)}
      <div transition:collapse|global class="expanded-box min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] bg-[#1c1c1e] border border-[#2c2c2e] rounded-lg p-2.5 mt-2 font-mono text-[11.5px] text-[var(--fg-1)]">
        <div class="text-[var(--fg-3)] mb-1 select-text">$ {@render shellCommand(item.command)}</div>
        <ToolBody text={item.run?.output || '(no output)'} />
      </div>
    {/if}
  </div>
{/snippet}

<!-- Normal finished ran row -->
{#snippet ranDone(item: any, idx: number)}
  <div class="ran-row my-0.5">
    <button
      type="button"
      class="flex w-full items-center gap-2 rounded px-2 py-1 text-left select-none hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[28px] sm:min-h-[28px] min-h-[44px]"
      onclick={() => toggleRow(`ran-${idx}`, false)}
    >
      <span class="text-[var(--fg-4)] shrink-0"><Icon name="terminal" size={12} /></span>
      <span class="font-medium text-[var(--fg-3)] shrink-0">Ran</span>
      <span class="font-mono text-[12.5px] text-[var(--fg-2)] truncate flex-1">{item.command}</span>
      {#if item.duration}
        <span class="text-xs text-[var(--fg-4)] tabular-nums shrink-0">{item.duration}</span>
      {/if}
    </button>

    {#if isRowExpanded(`ran-${idx}`, false)}
      <div transition:collapse|global class="expanded-box min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] bg-[#1c1c1e] border border-[#2c2c2e] rounded-lg p-2.5 my-1 font-mono text-[11.5px] text-[var(--fg-1)]">
        <div class="text-[var(--fg-3)] mb-1 select-text">$ {@render shellCommand(item.command)}</div>
        <ToolBody text={item.run?.output || '(no output)'} />
      </div>
    {/if}
  </div>
{/snippet}

<!-- Edit row -->
{#snippet editRow(item: any, idx: number)}
  <div class="edit-row my-0.5">
    <button
      type="button"
      class="flex w-full items-center gap-2 rounded px-2 py-1 text-left select-none hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[28px] sm:min-h-[28px] min-h-[44px]"
      onclick={() => toggleRow(`edit-${idx}`, false)}
    >
      <span class="text-[var(--tool-edit)] shrink-0"><Icon name="pencil" size={12} /></span>
      <span class="font-medium text-[var(--fg-3)] shrink-0">Edit</span>
      <span class="font-mono text-[12.5px] text-[var(--fg-2)] truncate flex-1">{item.path}</span>
      {#if item.additions > 0}
        <span class="font-mono text-xs text-[var(--status-done)] shrink-0">+{item.additions}</span>
      {/if}
      {#if item.deletions > 0}
        <span class="font-mono text-xs text-[var(--status-failed)] shrink-0">−{item.deletions}</span>
      {/if}
      {#if item.duration}
        <span class="text-xs text-[var(--fg-4)] tabular-nums shrink-0">{item.duration}</span>
      {/if}
    </button>

    {#if isRowExpanded(`edit-${idx}`, false)}
      <div transition:collapse|global class="expanded-box min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] bg-[#1c1c1e] border border-[#2c2c2e] rounded-lg p-2.5 my-1 font-mono text-[11.5px] text-[var(--fg-1)]">
        <ToolBody text={item.run?.display?.diff || item.run?.output || '(no diff)'} path={item.path} language="diff" />
      </div>
    {/if}
  </div>
{/snippet}

<!-- Standalone Thinking line -->
{#snippet thinkingRow(item: any, idx: number)}
  {@render thinkingBlock(item.text, `think-${idx}`, 'pl-3 py-1', 'text-[12.5px]')}
{/snippet}

<!-- Thinking block: clamped to 2 lines with Show more / Show less -->
{#snippet thinkingBlock(raw: any, key: any, pad: any, size: any)}
  {@const text = String(raw ?? '').trim()}
  {@const lines = text.split('\n')}
  {@const open = isRowExpanded(key, false)}
  <div class="thinking-block border-l-2 border-[var(--tool-thinking)]/40 {pad} my-1 text-xs select-text [overflow-wrap:anywhere]">
    <div class="flex items-center gap-1.5 font-medium text-[var(--tool-thinking)] mb-0.5">
      <Icon name="lightbulb" size={12} class="shrink-0" />
      <span>Thinking</span>
      {#if lines.length > 1}
        <span class="text-[var(--fg-4)] font-normal text-[11px]">· {lines.length} lines</span>
      {/if}
    </div>
    {#if lines.length < 3}
      <div class="text-[var(--fg-3)] {size} leading-5 whitespace-pre-wrap">{text}</div>
    {:else}
      <div class="text-[var(--fg-3)] {size} leading-5 whitespace-pre-wrap {open ? 'max-h-[200px] overflow-y-auto pr-1' : 'line-clamp-2'}">{open ? text : lines.slice(0, 2).join('\n')}</div>
      <button
        type="button"
        class="text-[11px] font-medium text-[var(--fg-4)] hover:text-[var(--fg-2)] mt-0.5 flex items-center gap-1 focus-visible:outline-none"
        onclick={() => toggleRow(key, false)}
      >
        {open ? 'Show less' : 'Show more'} <Icon name="chevron-down" size={11} class={open ? 'rotate-180' : ''} />
      </button>
    {/if}
  </div>
{/snippet}

{#snippet groupCount(n: number, icon: IconName, tone: string, one: string, many: string)}
  {#if n}
    <span class="inline-flex items-center gap-1 shrink-0">
      <Icon name={icon} size={12} class="{tone} shrink-0" />
      <span>{n} <span class="hidden sm:inline">{n === 1 ? one : many}</span></span>
    </span>
  {/if}
{/snippet}

<!-- ACP Fold / Restore and generic MCP rows share one shell -->
{#snippet flatRow(item: any, key: string, language: string = '')}
  <div class="my-0.5">
    <button
      type="button"
      class="flex w-full items-center gap-2 rounded px-2 py-1 text-left select-none hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[28px] sm:min-h-[28px] min-h-[44px]"
      onclick={() => toggleRow(key, false)}
    >
      {#if item.type === 'mcp'}
        <span class="h-[7px] w-[7px] rounded-full bg-[#93c5fd] shrink-0" aria-hidden="true"></span>
        <span class="font-mono text-[12px] text-[#93c5fd] shrink-0">{item.server}</span>
        <span class="text-[var(--fg-4)] select-none shrink-0">/</span>
        <span class="font-medium text-[var(--fg-3)] shrink-0">{item.tool}</span>
      {:else if item.type === 'compress'}
        <span class="h-[7px] w-[7px] rounded-full bg-[var(--tool-compress)] shrink-0" aria-hidden="true"></span>
        <span class="font-medium text-[var(--fg-3)] shrink-0">Fold</span>
      {:else}
        <span class="text-[var(--tool-read)] shrink-0"><Icon name="file-text" size={13} /></span>
        <span class="font-medium text-[var(--fg-3)] shrink-0">Restore</span>
      {/if}
      <span class="font-mono text-[12.5px] text-[var(--fg-2)] truncate flex-1">{item.target}</span>
      {#if item.duration}
        <span class="text-xs text-[var(--fg-4)] tabular-nums shrink-0">{item.duration}</span>
      {/if}
    </button>
    {#if isRowExpanded(key, false)}
      <div transition:collapse|global class="expanded-box min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] bg-[#1c1c1e] border border-[#2c2c2e] rounded-lg p-2.5 my-1 font-mono text-[11.5px] text-[var(--fg-1)]">
        <ToolBody text={item.run?.output || '(no output)'} {language} />
      </div>
    {/if}
  </div>
{/snippet}

{#snippet foldRow(item: any, idx: number)}{@render flatRow(item, `compress-${idx}`)}{/snippet}
{#snippet restoreRow(item: any, idx: number)}{@render flatRow(item, `decompress-${idx}`)}{/snippet}
{#snippet mcpRow(item: any, idx: number)}{@render flatRow(item, `mcp-${idx}`, 'json')}{/snippet}

<!-- Web search row -->
{#snippet webRow(item: any, idx: number)}
  <div class="web-row my-0.5">
    <button
      type="button"
      class="flex w-full items-center gap-2 rounded px-2 py-1 text-left select-none hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[28px] sm:min-h-[28px] min-h-[44px]"
      onclick={() => toggleRow(`web-${idx}`, false)}
    >
      <span class="text-[var(--fg-4)] shrink-0"><Icon name="globe" size={12} /></span>
      <span class="font-medium text-[var(--fg-3)] shrink-0">Web</span>
      <span class="font-mono text-[12.5px] text-[var(--fg-2)] truncate flex-1">{item.title}</span>
      {#if item.duration}
        <span class="text-xs text-[var(--fg-4)] tabular-nums shrink-0">{item.duration}</span>
      {/if}
    </button>

    {#if isRowExpanded(`web-${idx}`, false)}
      <div transition:collapse|global class="expanded-box min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] bg-[#1c1c1e] border border-[#2c2c2e] rounded-lg p-2.5 my-1 font-mono text-[11.5px] text-[var(--fg-1)]">
        <ToolBody text={item.run?.output || '(no output)'} />
      </div>
    {/if}
  </div>
{/snippet}

<!-- Agent row -->
{#snippet agentRow(item: any, idx: number)}
  <div class="agent-row my-0.5">
    <button
      type="button"
      class="flex w-full items-center gap-2 rounded px-2 py-1 text-left select-none hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[28px] sm:min-h-[28px] min-h-[44px]"
      onclick={() => toggleRow(`agent-${idx}`, false)}
    >
      <span class="text-[var(--fg-4)] shrink-0"><Icon name="bot" size={12} /></span>
      <span class="font-medium text-[var(--fg-3)] shrink-0">Agent</span>
      <span class="font-mono text-[12.5px] text-[var(--fg-2)] truncate flex-1">{item.title}</span>
      {#if item.duration}
        <span class="text-xs text-[var(--fg-4)] tabular-nums shrink-0">{item.duration}</span>
      {/if}
    </button>

    {#if isRowExpanded(`agent-${idx}`, false)}
      <div transition:collapse|global class="expanded-box min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] bg-[#1c1c1e] border border-[#2c2c2e] rounded-lg p-2.5 my-1 font-mono text-[11.5px] text-[var(--fg-1)]">
        <ToolBody text={item.run?.output || '(no output)'} />
      </div>
    {/if}
  </div>
{/snippet}

<!-- Background pill row -->
{#snippet backgroundRow(item: any, idx: number)}
  <div class="background-row my-0.5">
    <button
      type="button"
      class="flex w-full items-center gap-2 rounded px-2 py-1 text-left select-none hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[28px] sm:min-h-[28px] min-h-[44px]"
      onclick={() => toggleRow(`bg-${idx}`, false)}
    >
      <span class="text-[var(--fg-4)] shrink-0"><Icon name="terminal" size={12} /></span>
      <span class="font-medium text-[var(--fg-3)] shrink-0">Background</span>
      <span class="font-mono text-[12.5px] text-[var(--fg-2)] truncate flex-1">{item.title}</span>
    </button>
  </div>
{/snippet}

<!-- Execute row -->
{#snippet executeRow(item: any, idx: number)}
  <div class="execute-row my-0.5" data-testid="toolfeed-execute-row">
    <button
      type="button"
      class="flex w-full items-center gap-2 rounded px-2 py-1 text-left select-none hover:bg-[var(--surface-2)] focus-visible:ring-1 focus-visible:ring-[var(--border)] focus-visible:outline-none min-h-[28px] sm:min-h-[28px] min-h-[44px]"
      onclick={() => toggleRow(`exec-${idx}`, item.running)}
    >
      <span class="text-[var(--fg-4)] shrink-0"><Icon name="terminal" size={12} /></span>
      <span class="font-medium text-[var(--fg-3)] shrink-0">Execute</span>
      {#if item.summary}
        <span class="font-mono text-[12.5px] text-[var(--fg-2)] truncate flex-1">{item.summary}</span>
      {:else}
        <span class="font-mono text-[12.5px] text-[var(--fg-2)] truncate flex-1">{item.command}</span>
      {/if}
      {#if item.mutationStats && item.mutationStats.length > 0}
        <span class="flex items-center gap-1.5 shrink-0 text-xs font-mono">
          {#each item.mutationStats as stat, sIdx (sIdx)}
            <span class="text-[var(--fg-3)] truncate max-w-[140px]">{stat.path}</span>
            <span class="text-emerald-400">+{stat.additions}</span>
            <span class="text-rose-400">−{stat.deletions}</span>
          {/each}
        </span>
      {/if}
      {#if item.duration}
        <span class="text-xs text-[var(--fg-4)] tabular-nums shrink-0">{item.duration}</span>
      {/if}
    </button>

    {#if isRowExpanded(`exec-${idx}`, item.running)}
      {@render executeExpanded(item)}
    {/if}
  </div>
{/snippet}

{#snippet executeExpanded(item: any)}
  <div transition:collapse|global class="expanded-box min-w-0 max-w-full overflow-x-auto [overflow-wrap:anywhere] bg-[#1c1c1e] border border-[#2c2c2e] rounded-lg p-2.5 my-1 font-mono text-[11.5px] text-[var(--fg-1)]">
    {#if item.children && item.children.length > 0}
      <div class="space-y-1.5 mb-2.5">
        {#each item.children as child (child.toolId || child.key)}
          {@render execChild(child)}
        {/each}
      </div>
    {/if}

    {#if item.code}
      <div class="mt-2">
        <ToolBody text={item.code} language="code" />
      </div>
    {:else if item.run?.input}
      <pre class="whitespace-pre-wrap text-[var(--fg-3)] mb-2">{item.run.input}</pre>
    {/if}

    {#if item.run?.output}
      <div class="border-t border-[var(--border)] pt-2 mt-2">
        <ToolBody text={item.run.output} />
      </div>
    {/if}
  </div>
{/snippet}

{#snippet execChild(child: any)}
  <div class="child-row border-l-2 border-[var(--border)] pl-2 py-0.5 text-xs">
    <div class="flex items-center gap-1.5 text-[var(--fg-2)] font-mono">
      <span class="font-semibold text-[var(--fg-3)]">{bareName(child.name)}</span>
      {#if child.overlap}
        <span class="px-1 py-0.2 rounded text-[10px] bg-[var(--surface-3)] text-muted-foreground">parallel</span>
      {/if}
      <span class="text-[var(--fg-4)] truncate flex-1">{child.input}</span>
      {#if child.status === 'running'}
        <span class="text-status-working animate-pulse text-[11px]">running…</span>
      {:else if child.status === 'failed'}
        <span class="text-status-failed text-[11px]">failed</span>
      {/if}
    </div>
    {#if child.display?.diff}
      <div class="mt-1 text-[11px] font-mono whitespace-pre-wrap bg-[#111] p-1.5 rounded">
        <ToolBody text={child.display.diff} />
      </div>
    {:else if child.output}
      <div class="mt-1 text-[11px] font-mono whitespace-pre-wrap bg-[#111] p-1.5 rounded text-[var(--fg-3)] max-h-40 overflow-auto">
        <ToolBody text={child.output} />
      </div>
    {/if}
  </div>
{/snippet}

<style>
  .tool-feed {
    padding: 12px;
    background-color: var(--surface-0);
    border-radius: 8px;
  }

  @media (max-width: 640px) {
    .tool-feed {
      padding: 8px;
    }
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
