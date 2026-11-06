<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { rise } from '$lib/motion';
  import type { ProviderView } from '$lib/api/types';

  let providers = $state<ProviderView[]>([]);
  let error = $state('');
  let loading = $state(false);

  const subscription = (id: string) => id === 'agy' || id === 'openai-codex';

  async function refresh() {
    loading = true;
    error = '';
    try {
      providers = await api.listProviders();
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
          Subscription windows and key balances. Cached values; refresh is read-only.
        </p>
      </div>
      <button class="btn" disabled={loading} onclick={() => void refresh()}>
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

        {#if provider.signedIn}
          <div class="mt-4 grid gap-3 sm:grid-cols-2">
            <div class="rounded-xl border border-border bg-background p-3">
              <div class="flex items-center justify-between text-[12px] text-fg-3">
                <span>5-hour</span><span>—</span>
              </div>
              <div class="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-3">
                <div class="h-full w-0 rounded-full bg-green-500"></div>
              </div>
              <div class="mt-2 text-[11px] text-fg-3">Usage source not wired yet.</div>
            </div>
            <div class="rounded-xl border border-border bg-background p-3">
              <div class="flex items-center justify-between text-[12px] text-fg-3">
                <span>Weekly</span><span>—</span>
              </div>
              <div class="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-3">
                <div class="h-full w-0 rounded-full bg-green-500"></div>
              </div>
              <div class="mt-2 text-[11px] text-fg-3">Usage source not wired yet.</div>
            </div>
          </div>
        {:else}
          <p class="mt-3 text-[13px] text-fg-3">Connect this provider from Providers to read usage.</p>
        {/if}
      </div>
    {/each}

    {#if providers.length === 0 && !loading}
      <p class="text-[13px] text-fg-3">No providers reported by the daemon.</p>
    {/if}
  </div>
</section>
