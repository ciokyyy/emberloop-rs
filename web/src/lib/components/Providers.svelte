<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { api } from '$lib/api';
  import { fade } from 'svelte/transition';
  import { flip, flipAnimation, flipReady, motionParams, rise, rowEnter, slide, slideDown } from '$lib/motion';
  import type { LoginView, Model, ProviderView } from '$lib/api/types';

  // The Session shell owns the model list; a new credential changes it.
  let { onmodels = () => {} }: { onmodels?: (models: Model[]) => void } = $props();

  let providers = $state<ProviderView[]>([]);
  let login = $state<LoginView | null>(null);
  let codex = $state<LoginView | null>(null);
  let claude = $state<LoginView | null>(null);
  let code = $state('');
  let label = $state('');
  let address = $state('');
  let search = $state('');
  let picked = $state<string | null>(null);
  let pickedKey = $state('');
  let error = $state('');
  let loading = $state(false);
  let copied = $state(false);
  let poll: ReturnType<typeof setInterval> | null = null;
  let codexPoll: ReturnType<typeof setInterval> | null = null;
  let claudePoll: ReturnType<typeof setInterval> | null = null;

  async function refresh() {
    try {
      providers = await api.listProviders();
    } catch (cause) {
      error = String(cause);
    }
  }

  function watch(id: string) {
    stopWatch();
    poll = setInterval(() => {
      void (async () => {
        try {
          login = await api.getAgyLogin(id);
          if (login.state !== 'running') {
            stopWatch();
            void refresh();
          }
        } catch (cause) {
          error = String(cause);
          stopWatch();
        }
      })();
    }, 1000);
  }

  function stopWatch() {
    if (poll) clearInterval(poll);
    poll = null;
  }

  function watchCodex(id: string) {
    stopCodexWatch();
    codexPoll = setInterval(() => {
      void (async () => {
        try {
          codex = await api.getCodexLogin(id);
          if (codex.state !== 'running') {
            stopCodexWatch();
            await refresh();
            if (codex.state === 'succeeded') onmodels(await api.refreshModels());
          }
        } catch (cause) {
          error = String(cause);
          stopCodexWatch();
        }
      })();
    }, 1000);
  }

  function stopCodexWatch() {
    if (codexPoll) clearInterval(codexPoll);
    codexPoll = null;
  }

  function watchClaude(id: string) {
    stopClaudeWatch();
    claudePoll = setInterval(() => {
      void (async () => {
        try {
          claude = await api.getClaudeLogin(id);
          if (claude.state !== 'running') {
            stopClaudeWatch();
            await refresh();
            if (claude.state === 'succeeded') onmodels(await api.refreshModels());
          }
        } catch (cause) {
          error = String(cause);
          stopClaudeWatch();
        }
      })();
    }, 1000);
  }

  function stopClaudeWatch() {
    if (claudePoll) clearInterval(claudePoll);
    claudePoll = null;
  }

  async function startClaude() {
    loading = true;
    error = '';
    try {
      claude = await api.startClaudeLogin();
      code = '';
      watchClaude(claude.id);
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  }

  async function completeClaude() {
    if (!claude) return;
    loading = true;
    error = '';
    try {
      claude = await api.completeClaudeLogin(claude.id, code);
      code = '';
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  }

  async function cancelClaude() {
    if (!claude) return;
    stopClaudeWatch();
    try {
      await api.cancelClaudeLogin(claude.id);
      claude = null;
      await refresh();
    } catch (cause) {
      error = String(cause);
    }
  }

  async function startCodex() {
    loading = true;
    error = '';
    try {
      codex = await api.startCodexLogin();
      watchCodex(codex.id);
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  }

  async function cancelCodex() {
    if (!codex) return;
    stopCodexWatch();
    try {
      await api.cancelCodexLogin(codex.id);
      codex = null;
      await refresh();
    } catch (cause) {
      error = String(cause);
    }
  }

  async function signIn() {
    loading = true;
    error = '';
    try {
      login = await api.startAgyLogin(label);
      address = '';
      watch(login.id);
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  }

  async function complete() {
    if (!login) return;
    loading = true;
    error = '';
    try {
      login = await api.completeAgyLogin(login.id, address);
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  }

  async function cancel() {
    if (!login) return;
    stopWatch();
    try {
      await api.cancelAgyLogin(login.id);
      login = null;
      await refresh();
    } catch (cause) {
      error = String(cause);
    }
  }

  async function signOut(id: string) {
    if (!window.confirm('Remove this Account? Its saved credentials are deleted.')) return;
    try {
      await api.signOutAccount(id);
      await refresh();
    } catch (cause) {
      error = String(cause);
    }
  }

  const connected = $derived(providers.filter((p) => p.signedIn || p.auth === 'login'));
  const addable = $derived(
    providers.filter(
      (p) => p.auth === 'key' && !p.signedIn && p.name.toLowerCase().includes(search.trim().toLowerCase())
    )
  );

  async function saveKey(id: string) {
    loading = true;
    error = '';
    try {
      await api.setProviderKey(id, pickedKey);
      pickedKey = '';
      picked = null;
      await refresh();
      onmodels(await api.refreshModels());
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  }

  async function copy() {
    if (!login?.url) return;
    await navigator.clipboard.writeText(login.url);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  onMount(() => {
    void refresh();
    return () => {
      stopWatch();
      stopCodexWatch();
      stopClaudeWatch();
    };
  });
  onDestroy(() => {
    stopWatch();
    stopCodexWatch();
    stopClaudeWatch();
  });
</script>

<section class="h-full overflow-y-auto p-6" in:rise|global>
  <h2 class="text-[1.75rem] font-semibold leading-[1.2] tracking-[-0.02em]">Providers</h2>
  <p class="mt-1 text-[13px] text-fg-3">
    The daemon spawns the Antigravity server for the Account's own home. No token passes through
    Emberloop.
  </p>

  {#if error}
    <p
      class="mt-4 rounded border border-status-failed/40 bg-status-failed/10 px-3 py-2 text-[13px]"
      transition:slideDown
    >
      {error}
    </p>
  {/if}

  {#each connected as provider (provider.id)}
    <div class="mt-6 rounded border border-border" in:rise>
      <div class="flex items-center justify-between border-b border-border px-4 py-3">
        <div>
          <div class="font-medium">{provider.name}</div>
          <div class="text-[12px] text-fg-3">
            {provider.signedIn ? 'Signed in' : 'Signed out'}
          </div>
        </div>
        {#if provider.id === 'openai-codex' && !provider.signedIn}
          <button
            class="btn"
            disabled={loading || codex?.state === 'running'}
            onclick={() => void startCodex()}
          >
            Connect
          </button>
        {/if}
      </div>
      <ul class="divide-y divide-border">
        {#each provider.accounts as account (account.id)}
          <li
            class="flex items-center justify-between px-4 py-3"
            use:flipReady
            animate:flip={flipAnimation()}
            in:slide={rowEnter}
            out:fade={motionParams()}
          >
            <div>
              <div>{account.label}</div>
              <div class="text-[12px] text-fg-3">
                {account.signedIn ? `Tier ${account.tier || 'unknown'}` : 'Not signed in'}
              </div>
            </div>
            {#if account.signedIn}
              <button class="btn" onclick={() => void signOut(account.id)}>
                {provider.auth === 'key' ? 'Remove key' : 'Sign out'}
              </button>
            {/if}
          </li>
        {/each}
        {#if provider.accounts.length === 0}
          <li class="px-4 py-3 text-[13px] text-fg-3">No Account yet.</li>
        {/if}
      </ul>
    </div>
  {/each}

  {#if codex}
    <div class="mt-6 rounded border border-border p-4">
      <div class="flex items-center justify-between">
        <div class="font-medium">
          {#if codex.state === 'running'}Connect ChatGPT
          {:else if codex.state === 'succeeded'}ChatGPT connected
          {:else if codex.state === 'canceled'}Login canceled
          {:else}Login failed{/if}
        </div>
        {#if codex.state === 'running'}
          <button class="btn" onclick={() => void cancelCodex()}>Cancel</button>
        {/if}
      </div>

      {#if codex.state === 'running'}
        <p class="mt-3 text-[13px] text-fg-3">
          Open
          <a class="underline" href={codex.verificationUri} target="_blank" rel="noreferrer">
            {codex.verificationUri}
          </a>
          and enter this code.
        </p>
        <code class="mt-2 block w-fit rounded bg-surface-3 px-3 py-2 font-mono text-lg tracking-wider">
          {codex.userCode}
        </code>
        {#if codex.expiresAt}
          <p class="mt-2 text-[12px] text-fg-3">
            Expires {new Date(codex.expiresAt).toLocaleTimeString()}.
          </p>
        {/if}
      {:else if codex.state === 'succeeded'}
        <p class="mt-2 text-[13px] text-fg-3">The ChatGPT OAuth credential is saved for pi Sessions.</p>
      {:else if codex.error}
        <p class="mt-2 text-[13px] text-status-failed">{codex.error}</p>
      {/if}
    </div>
  {/if}


  {#if claude}
    <div class="mt-6 rounded border border-border p-4">
      <div class="flex items-center justify-between">
        <div class="font-medium">
          {#if claude.state === 'running'}Connect Claude
          {:else if claude.state === 'succeeded'}Claude connected
          {:else if claude.state === 'canceled'}Login canceled
          {:else}Login failed{/if}
        </div>
        {#if claude.state === 'running'}
          <button class="btn" onclick={() => void cancelClaude()}>Cancel</button>
        {/if}
      </div>

      {#if claude.state === 'running'}
        {#if claude.url}
          <p class="mt-3 text-[13px] text-fg-3">
            Open
            <a class="underline" href={claude.url} target="_blank" rel="noreferrer">
              the Claude sign-in page
            </a>
            and approve access. Then paste the code the page shows.
          </p>
          <div class="mt-2 flex gap-2">
            <input class="input min-w-0 flex-1" placeholder="Paste code" bind:value={code} />
            <button class="btn" disabled={loading || !code.trim()} onclick={() => void completeClaude()}>
              Complete
            </button>
          </div>
        {:else}
          <p class="mt-3 text-[13px] text-fg-3">Waiting for the CLI to print the sign-in link…</p>
        {/if}
      {:else if claude.state === 'succeeded'}
        <p class="mt-2 text-[13px] text-fg-3">
          Tier {claude.tier || 'unknown'}. Claude Sessions copy this credential into their own home.
        </p>
      {:else if claude.error}
        <p class="mt-2 text-[13px] text-status-failed">{claude.error}</p>
      {:else if claude.output}
        <pre class="mt-2 overflow-x-auto text-[12px] text-fg-3">{claude.output}</pre>
      {/if}
    </div>
  {:else}
    <div class="mt-6 rounded border border-border p-4">
      <div class="font-medium">Sign in to Claude</div>
      <p class="mt-1 text-[12px] text-fg-3">
        Runs `claude auth login` in the daemon's own Claude home. The credential stays there.
      </p>
      <button class="btn mt-2" disabled={loading} onclick={() => void startClaude()}>Sign in</button>
    </div>
  {/if}

  {#if login}
    <div class="mt-6 rounded border border-border p-4">
      <div class="flex items-center justify-between">
        <div class="font-medium">
          {#if login.state === 'running'}Waiting for sign-in
          {:else if login.state === 'succeeded'}Signed in
          {:else if login.state === 'canceled'}Login canceled
          {:else}Login failed{/if}
        </div>
        {#if login.state === 'running'}
          <button class="btn" onclick={() => void cancel()}>Cancel</button>
        {/if}
      </div>

      {#if login.state === 'running'}
        {#if login.url}
          <div class="mt-3 flex flex-wrap items-center gap-2">
            <a class="btn" href={login.url} target="_blank" rel="noreferrer">Open sign-in page</a>
            <button class="btn" onclick={() => void copy()}>{copied ? 'Copied' : 'Copy link'}</button>
          </div>
          <p class="mt-3 text-[13px] text-fg-3">
            After signing in, the browser lands on a <code>127.0.0.1</code> address that does not
            load on a remote machine. Paste that address here.
          </p>
          <div class="mt-2 flex gap-2">
            <input
              class="input min-w-0 flex-1"
              placeholder="http://127.0.0.1:…"
              bind:value={address}
            />
            <button class="btn" disabled={loading || !address} onclick={() => void complete()}>
              Complete
            </button>
          </div>
        {:else}
          <p class="mt-3 text-[13px] text-fg-3">Waiting for the server to print the sign-in link…</p>
        {/if}
      {:else if login.state === 'succeeded'}
        <p class="mt-2 text-[13px] text-fg-3">
          Tier {login.tier || 'unknown'}. The Account is kept and ready for Sessions.
        </p>
      {:else if login.error}
        <p class="mt-2 text-[13px] text-status-failed">{login.error}</p>
      {/if}
    </div>
  {:else}
    <div class="mt-6 rounded border border-border p-4">
      <div class="font-medium">Sign in to Antigravity</div>
      <div class="mt-2 flex gap-2">
        <input class="input min-w-0 flex-1" placeholder="Account label (optional)" bind:value={label} />
        <button class="btn" disabled={loading} onclick={() => void signIn()}>Sign in</button>
      </div>
    </div>
  {/if}

  <div class="mt-6 rounded border border-border p-4">
    <div class="font-medium">Add provider</div>
    <p class="mt-1 text-[12px] text-fg-3">
      Paste an API key. pi Sessions reach the provider through the proxy; the key stays in the
      daemon's keys file.
    </p>
    <input
      class="input mt-2 w-full"
      placeholder="Search providers"
      aria-label="Search providers"
      bind:value={search}
    />
    <ul class="mt-2 max-h-72 divide-y divide-border overflow-y-auto">
      {#each addable as provider (provider.id)}
        <li class="py-2" in:slide={rowEnter}>
          <button
            class="row w-full px-2 py-1.5 text-left text-[14px]"
            onclick={() => ((picked = picked === provider.id ? null : provider.id), (pickedKey = ''))}
          >
            {provider.name}
          </button>
          {#if picked === provider.id}
            <div class="mt-2 flex gap-2 px-2" transition:slideDown>
              <input
                class="input min-w-0 flex-1"
                type="password"
                placeholder="{provider.name} API key"
                bind:value={pickedKey}
              />
              <button
                class="btn"
                disabled={loading || !pickedKey.trim()}
                onclick={() => void saveKey(provider.id)}
              >
                Save key
              </button>
            </div>
          {/if}
        </li>
      {/each}
      {#if addable.length === 0}
        <li class="py-2 text-[13px] text-fg-3">No provider matches.</li>
      {/if}
    </ul>
  </div>
</section>
