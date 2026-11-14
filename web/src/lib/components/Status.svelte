<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { SystemStatus, TokenStatus } from '$lib/api/types';
  import { rise } from '$lib/motion';
  import StatusSystem from './StatusSystem.svelte';
  import StatusTokens from './StatusTokens.svelte';

  type StatusTab = 'system' | 'tokens';

  let tab = $state<StatusTab>('system');
  let live = $state(true);

  let systemData = $state<SystemStatus | null>(null);
  let systemLoading = $state(false);
  let systemError = $state('');

  let tokenData = $state<TokenStatus | null>(null);
  let selectedMonth = $state<string | undefined>(undefined);
  let tokenLoading = $state(false);
  let tokenError = $state('');

  async function loadSystem(refresh = false) {
    if (systemLoading && !refresh) return;
    systemLoading = true;
    systemError = '';
    try {
      systemData = await api.systemStatus(refresh);
    } catch (e) {
      systemError = e instanceof Error ? e.message : 'Failed to load system status.';
    } finally {
      systemLoading = false;
    }
  }

  async function loadTokens(month?: string) {
    tokenLoading = true;
    tokenError = '';
    try {
      const next = await api.tokenStatus(month);
      tokenData = next;
      selectedMonth = next.month;
    } catch (e) {
      tokenError = e instanceof Error ? e.message : 'Failed to load token telemetry.';
    } finally {
      tokenLoading = false;
    }
  }

  function switchTab(next: StatusTab) {
    tab = next;
    if (next === 'tokens' && !tokenData && !tokenLoading) {
      void loadTokens(selectedMonth);
    }
  }

  onMount(() => {
    void loadSystem(false);
  });

  $effect(() => {
    if (tab !== 'system' || !live) return;
    const timer = setInterval(() => {
      void loadSystem(true);
    }, 5000);
    return () => clearInterval(timer);
  });
</script>

<section class="h-full overflow-y-auto p-4 md:p-6" in:rise|global>
  <div class="mx-auto flex max-w-[1040px] flex-col gap-3">
    <div class="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-3">
      <div
        class="inline-flex rounded-xl border border-border bg-surface-2 p-1"
        role="tablist"
        aria-label="Status sections"
      >
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'system'}
          class="rounded-lg px-3.5 py-1.5 text-[12px] font-medium transition-colors {tab === 'system'
            ? 'bg-surface-3 text-fg-1'
            : 'text-fg-3 hover:text-fg-1'}"
          onclick={() => switchTab('system')}
        >
          System
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'tokens'}
          class="rounded-lg px-3.5 py-1.5 text-[12px] font-medium transition-colors {tab === 'tokens'
            ? 'bg-surface-3 text-fg-1'
            : 'text-fg-3 hover:text-fg-1'}"
          onclick={() => switchTab('tokens')}
        >
          Tokens
        </button>
      </div>

      <span class="text-[11px] text-fg-3">
        {tab === 'system' ? 'Host, process & storage telemetry' : 'Subscription token & latency ledger'}
      </span>
    </div>

    {#if tab === 'system'}
      {#if systemError}
        <p class="rounded-lg border border-status-failed/40 bg-status-failed/10 px-3 py-2 text-[13px] text-[#ffb3ad]">
          {systemError}
        </p>
      {/if}
      {#if systemData}
        <StatusSystem
          data={systemData}
          loading={systemLoading}
          {live}
          ontogglelive={() => (live = !live)}
          onrefresh={() => void loadSystem(true)}
        />
      {:else if !systemError}
        <div class="grid min-h-[240px] place-items-center text-[13px] text-fg-3">
          Loading system status…
        </div>
      {/if}
    {:else}
      {#if tokenError}
        <p class="rounded-lg border border-status-failed/40 bg-status-failed/10 px-3 py-2 text-[13px] text-[#ffb3ad]">
          {tokenError}
        </p>
      {/if}
      {#if tokenData}
        <StatusTokens
          data={tokenData}
          loading={tokenLoading}
          onmonthchange={(month) => void loadTokens(month)}
          onrefresh={() => void loadTokens(selectedMonth)}
        />
      {:else if !tokenError}
        <div class="grid min-h-[240px] place-items-center text-[13px] text-fg-3">
          Loading token telemetry…
        </div>
      {/if}
    {/if}
  </div>
</section>
