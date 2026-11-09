<script lang="ts">
  import { api } from '$lib/api';
  import type { SessionStatus, UsageRecord } from '$lib/api/types';
  import { coldStart, latestHit, percentLabel, sessionHit, trendPoints } from '$lib/cache';
  import CacheTrend from './CacheTrend.svelte';
  import * as Table from './ui/table';

  // The table shows the newest requests; the trend and the Session figure use every row loaded.
  const RECENT = 10;
  // The server's default history window; a full window means older requests are not shown.
  const HISTORY = 200;

  let { sessionId, status = 'idle' }: { sessionId: string; status?: SessionStatus } = $props();

  let rows = $state<UsageRecord[] | null>(null);
  let failure = $state('');
  let shownFor = '';

  // Loads the history of a Session, then again whenever its status changes, so a turn that just
  // ended shows its requests. Another Session's rows never stay on screen.
  $effect(() => {
    const id = sessionId;
    void status;
    let current = true;
    if (id !== shownFor) rows = null;
    shownFor = id;
    failure = '';
    api
      .sessionUsage(id, HISTORY)
      .then((loaded) => {
        if (current) rows = loaded;
      })
      .catch((cause: unknown) => {
        if (current) failure = cause instanceof Error ? cause.message : String(cause);
      });
    return () => {
      current = false;
    };
  });

  const loaded = $derived(rows ?? []);
  const reported = $derived(loaded.some((row) => row.hitPercent !== null));
  const latest = $derived(latestHit(loaded));
  const session = $derived(sessionHit(loaded));
  const points = $derived(trendPoints(loaded));
  const recent = $derived(loaded.slice(-RECENT).reverse());
  const count = (value: number) => value.toLocaleString();
</script>

<div class="min-h-0 flex-1 overflow-y-auto px-4 py-4">
<div class="mx-auto flex max-w-[760px] flex-col gap-4">
<h2 class="text-[17px] font-semibold tracking-[-0.01em]">Prompt cache</h2>
{#if failure}
  <div class="p-4 text-[13px]" role="alert" style="color:#ffb3ad">{failure}</div>
{:else if rows === null}
  <div class="grid flex-1 place-items-center text-[13px] text-fg-3" role="status">Loading…</div>
{:else if loaded.length === 0}
  <p class="p-4 text-[13px] text-fg-3">
    No model requests yet. Each request's prompt-cache counts appear here once the provider answers.
  </p>
{:else if !reported}
  <p class="rounded-xl border border-dashed border-border p-4 text-[13px] text-fg-3">
    The provider reports no cache fields for this Session, so no hit rate is shown. Caching may be
    unsupported, or the prompts are below the provider's minimum prompt size for caching.
  </p>
{:else}
  <div class="flex flex-col gap-5">
    <section class="grid grid-cols-2 gap-3" aria-label="Prompt-cache hit rate">
      <div class="rounded-xl border border-border bg-surface-1 p-4">
        <p class="text-[12px] text-fg-3">Latest request</p>
        <p class="mt-1 text-4xl font-semibold tabular-nums" data-testid="latest-hit">{percentLabel(latest)}</p>
      </div>
      <div class="rounded-xl border border-border bg-surface-1 p-4">
        <p class="text-[12px] text-fg-3">Session, token-weighted</p>
        <p class="mt-1 text-4xl font-semibold tabular-nums" data-testid="session-hit">{percentLabel(session)}</p>
        <p class="mt-1 text-[12px] text-fg-3">
          {loaded.length >= HISTORY ? `Across the last ${HISTORY} requests` : `Across ${loaded.length} requests`}
        </p>
      </div>
    </section>

    {#if coldStart(loaded)}
      <p class="text-[12px] text-fg-3">
        The oldest request shown wrote the cache without reading it, so the early points start low.
      </p>
    {/if}

    <section class="flex flex-col gap-2" aria-label="Hit rate per request">
      <CacheTrend {points} />
    </section>

    <section class="flex flex-col gap-2" aria-label="Recent requests">
      <h3 class="text-[13px] font-medium">Last {recent.length} requests, newest first</h3>
      <Table.Root>
        <Table.Header>
          <Table.Row>
            <Table.Head>Request</Table.Head>
            <Table.Head class="text-right">Cached</Table.Head>
            <Table.Head class="text-right">Written</Table.Head>
            <Table.Head class="text-right">Fresh</Table.Head>
            <Table.Head class="text-right">Hit</Table.Head>
          </Table.Row>
        </Table.Header>
        <Table.Body>
          {#each recent as row (row.seq)}
            <Table.Row>
              <Table.Cell>
                #{row.seq}
                {#if row.fold}
                  <span
                    class="ml-2 rounded border border-border px-1 text-[10px] text-fg-3"
                    data-testid="fold-badge"
                    title="A compression rewrote the head of the conversation before this request, so the provider cache restarted."
                  >fold</span>
                {/if}
              </Table.Cell>
              <Table.Cell class="text-right tabular-nums">{count(row.cachedTokens)}</Table.Cell>
              <Table.Cell class="text-right tabular-nums">{count(row.cacheCreationTokens)}</Table.Cell>
              <Table.Cell class="text-right tabular-nums">{count(row.freshInputTokens)}</Table.Cell>
              <Table.Cell class="text-right tabular-nums">{percentLabel(row.hitPercent ?? null)}</Table.Cell>
            </Table.Row>
          {/each}
        </Table.Body>
      </Table.Root>
      <p class="text-[11px] text-fg-3">Tokens. Written: the provider wrote this part of the prompt to its cache. Fresh: the rest of the prompt, neither read nor written. fold: a compression rewrote the head of the conversation before this request, so the provider cache restarted from the first turn and this request paid full price.</p>
    </section>
  </div>
{/if}
</div>
</div>
