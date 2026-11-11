<script lang="ts">
  import Brand from './Brand.svelte';
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { rise } from '$lib/motion';
  import type { ProviderView, UsageView } from '$lib/api/types';

  let providers = $state<ProviderView[]>([]);
  let values = $state<Record<string, UsageView>>({});
  let error = $state('');
  let loading = $state(false);
  let refreshed = $state<number | null>(null);

  const subscription = (id: string) => id === 'agy' || id === 'openai-codex' || id === 'claude';

  // 70 and 90 match the daemon's pressure bands: the bar turns amber before a limit bites.
  const tone = (percent: number) => (percent >= 90 ? 'bg-status-failed' : percent >= 70 ? 'bg-status-needs' : 'bg-status-done');

  function ago(at: number) {
    const minutes = Math.floor((Date.now() - at) / 60000);
    return minutes < 1 ? 'just now' : minutes === 1 ? '1 minute ago' : `${minutes} minutes ago`;
  }

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
      refreshed = Date.now();
    }
  }

  onMount(() => void refresh());
</script>


<section class="h-full overflow-y-auto p-4" in:rise|global>
  <div class="mx-auto flex max-w-[720px] flex-col gap-3">
    <div class="flex h-9 items-center justify-between gap-3">
      <span class="text-[12px] text-fg-3">
        {refreshed ? `Refreshed ${ago(refreshed)} · cached 60 s` : 'Reading usage · cached 60 s'}
      </span>
      <button class="btn shrink-0" disabled={loading} onclick={() => void refresh(true)}>
        {loading ? 'Refreshing…' : 'Refresh'}
      </button>
    </div>

    {#if error}
      <p class="rounded-lg border border-status-failed/40 bg-status-failed/10 px-3 py-2 text-[13px]">{error}</p>
    {/if}

    {#each providers as provider (provider.id)}
      {@const usage = values[provider.id]}
      <div class="rounded-xl border border-border bg-surface-1">
        <div class="flex h-9 items-center justify-between gap-3 px-3.5">
          <div class="flex min-w-0 items-center gap-2">
            <Brand id={provider.id} size={20} />
            <span class="truncate text-[12px] font-semibold text-fg-1">{provider.name}</span>
            <span class="shrink-0 text-[10px] text-fg-3">
              {subscription(provider.id) ? 'Subscription' : 'API key'}
            </span>
          </div>
          <span class="flex h-[18px] shrink-0 items-center gap-1.5 rounded-full border border-border px-2 text-[10px] text-fg-3">
            <span class="size-1.5 rounded-full {provider.signedIn ? 'bg-status-done' : 'bg-fg-4'}"></span>
            {provider.signedIn ? 'Ready' : 'Off'}
          </span>
        </div>

        <div class="border-t border-border px-3.5 pb-2.5 pt-1">
          {#if !provider.signedIn}
            <p class="py-2 text-[12px] text-fg-3">Not connected. Connect it on Providers to read usage.</p>
          {:else if usage?.error}
            <p class="py-2 text-[12px] text-fg-3" role="status">{usage.error}</p>
          {:else if usage}
            {#each usage.windows as window}
              <div class="pt-1.5">
                <div class="flex items-baseline justify-between gap-2 text-[11px]">
                  <span class="text-fg-2">{window.label}</span>
                  <span class="tabular-nums text-fg-3">{Math.round(window.usedPercent)}% used</span>
                </div>
                <div
                  class="mt-1 h-1 overflow-hidden rounded-full bg-surface-3"
                  role="progressbar"
                  aria-label={window.label}
                  aria-valuenow={window.usedPercent}
                  aria-valuemin="0"
                  aria-valuemax="100"
                >
                  <div class="h-full rounded-full {tone(window.usedPercent)}" style:width={`${window.usedPercent}%`}></div>
                </div>
                {#if window.resetsAt}
                  <div class="mt-1 text-[10px] text-fg-4">Resets {resetLabel(window.resetsAt)}</div>
                {/if}
              </div>
            {/each}
            {#if usage.balance}
              <p class="pt-1.5 text-[18px] font-semibold tabular-nums">
                {usage.balance.currency} {usage.balance.remaining.toFixed(2)} remaining
              </p>
              <p class="text-[10px] text-fg-4">Balance read from the provider.</p>
            {/if}
          {:else}
            <p class="py-2 text-[12px] text-fg-3">Reading usage…</p>
          {/if}
        </div>
      </div>
    {/each}

    {#if providers.length === 0 && !loading}
      <p class="text-[12px] text-fg-3">No providers reported by the daemon.</p>
    {/if}
  </div>
</section>
