<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { rise } from '$lib/motion';
  import type { ProviderView, UsageView } from '$lib/api/types';

  let providers = $state<ProviderView[]>([]);
  let values = $state<Record<string, UsageView>>({});
  let error = $state('');
  let loading = $state(false);

  const subscription = (id: string) => id === 'agy' || id === 'openai-codex';

  function resetLabel(value: string) {
    const date = new Date(/^\d+$/.test(value) ? Number(value) * 1000 : value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
  }

  async function refresh(force = false) {
    loading = true;
    error = '';
    try {
      providers = (await api.listProviders()).filter(
        (provider) => provider.signedIn || provider.auth === 'login'
      );
      await Promise.all(providers.filter((provider) => provider.signedIn).map(async (provider) => {
        try {
          values[provider.id] = await api.providerUsage(provider.id, force);
        } catch {
          values[provider.id] = { windows: [], balance: null, error: 'Usage could not be read. Try refreshing.' };
        }
      }));
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  }

  onMount(() => void refresh());
</script>

<section class="h-full overflow-y-auto px-4 py-6" in:rise|global>
  <div class="mx-auto flex max-w-[760px] flex-col gap-5">
    <div class="flex items-start justify-between gap-3">
      <div>
        <h2 class="text-[1.75rem] font-semibold leading-[1.2] tracking-[-0.02em]">Usage</h2>
        <p class="text-[13px] text-fg-3">
          Subscription windows and key balances. Cached for 60 seconds. Refresh does not change saved credentials.
        </p>
      </div>
      <button class="btn" disabled={loading} onclick={() => void refresh(true)}>
        {loading ? 'Refreshing…' : 'Refresh'}
      </button>
    </div>

    {#if error}
      <p class="rounded border border-status-failed/40 bg-status-failed/10 px-3 py-2 text-[13px]">
        {error}
      </p>
    {/if}

    {#each providers as provider (provider.id)}
      <div class="rounded-2xl border border-border bg-surface-1 p-4">
        <div class="flex items-center justify-between gap-3">
          <div>
            <div class="font-medium">{provider.name}</div>
            <div class="text-[12px] text-fg-3">
              {subscription(provider.id) ? 'Subscription' : 'API key'} ·
              {provider.signedIn ? 'connected' : 'not connected'}
            </div>
          </div>
          <span class="chip">{provider.signedIn ? 'Ready' : 'Off'}</span>
        </div>

        {#if !provider.signedIn}
          <p class="mt-3 text-[13px] text-fg-3">Connect this provider from Providers to read usage.</p>
        {:else if values[provider.id]?.error}
          <p class="mt-3 text-[13px] text-fg-3" role="status">{values[provider.id].error}</p>
        {:else if values[provider.id]}
          {#if values[provider.id].balance}
            {@const balance = values[provider.id].balance!}
            <p class="mt-3 font-medium">{balance.currency} {balance.remaining.toFixed(2)} remaining</p>
          {/if}
          <div class="mt-4 grid gap-3 sm:grid-cols-2">
            {#each values[provider.id].windows as window}
              <div class="rounded-xl border border-border bg-background p-3">
                <div class="flex items-center justify-between gap-2 text-[12px] text-fg-3">
                  <span>{window.label}</span><span>{Math.round(window.usedPercent)}% used</span>
                </div>
                <div class="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-3" role="progressbar" aria-label={window.label} aria-valuenow={window.usedPercent} aria-valuemin="0" aria-valuemax="100">
                  <div class="h-full rounded-full bg-green-500" style:width={`${window.usedPercent}%`}></div>
                </div>
                {#if window.resetsAt}<div class="mt-2 text-[11px] text-fg-3">Resets {resetLabel(window.resetsAt)}</div>{/if}
              </div>
            {/each}
          </div>
        {:else}
          <p class="mt-3 text-[13px] text-fg-3">Reading usage…</p>
        {/if}
      </div>
    {/each}

    {#if providers.length === 0 && !loading}
      <p class="text-[13px] text-fg-3">No providers reported by the daemon.</p>
    {/if}
  </div>
</section>
