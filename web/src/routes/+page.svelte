<script lang="ts">
  import { onMount } from 'svelte';
  import { api, eventsUrl } from '$lib/api';
  import type {
    Model,
    Project,
    Session,
    SessionEvent,
    SessionStatus,
    Settings as SettingsDto
  } from '$lib/api/types';
  import { fadeAt, slideDown } from '$lib/motion';
  import Composer from '$lib/components/Composer.svelte';
  import Feed from '$lib/components/Feed.svelte';
  import NewSessionDialog from '$lib/components/NewSessionDialog.svelte';
  import Providers from '$lib/components/Providers.svelte';
  import Settings from '$lib/components/Settings.svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import { applyEvent, emptyFeedState, type FeedState } from '$lib/feed';
  import { toPromptImage } from '$lib/images';

  let projects = $state<Project[]>([]);
  let sessions = $state<Session[]>([]);
  let models = $state<Model[]>([]);
  let settings = $state<SettingsDto>({ proxy_compression: true, ponytail_default_level: 'full' });
  let selected = $state<string | null>(null);
  let feed = $state<FeedState>(emptyFeedState());
  let statuses = $state<Record<string, SessionStatus>>({});
  let view = $state<'session' | 'settings' | 'providers'>('session');
  let loading = $state(true);
  let error = $state('');
  let newOpen = $state(false);
  let drawer = $state(false);
  let settingsSaving = $state(false);

  let source: EventSource | null = null;
  let retry: ReturnType<typeof setTimeout> | null = null;

  const current = $derived(sessions.find((session) => session.id === selected) ?? null);
  const projectOf = $derived(projects.find((project) => project.id === current?.projectId) ?? null);
  const working = $derived(feed.status === 'working');
  const usagePct = $derived(
    feed.usage ? Math.min(100, Math.round((feed.usage.used / feed.usage.window) * 100)) : 0
  );
  const usageLabel = $derived(
    feed.usage
      ? `${Math.round(feed.usage.used / 1000)}k / ${Math.round(feed.usage.window / 1_000_000)}M`
      : ''
  );
  const statusWord: Record<SessionStatus, string> = {
    idle: 'Idle',
    working: 'Working',
    failed: 'Failed',
    paused: 'Paused'
  };
  const statusDot: Record<SessionStatus, string> = {
    idle: 'bg-status-done',
    working: 'bg-status-working',
    failed: 'bg-status-failed',
    paused: 'bg-status-paused'
  };

  const fail = (cause: unknown) => (error = cause instanceof Error ? cause.message : String(cause));

  onMount(() => {
    void boot();
    return () => closeStream();
  });

  async function boot() {
    try {
      const [loadedProjects, loadedSessions, loadedModels, loadedSettings] = await Promise.all([
        api.listProjects(),
        api.listSessions(),
        api.listModels(),
        api.getSettings()
      ]);
      projects = loadedProjects;
      sessions = loadedSessions;
      models = loadedModels;
      settings = loadedSettings;
      const latest = [...loadedSessions].sort((a, b) => b.updatedAt - a.updatedAt)[0];
      if (latest) select(latest.id);
    } catch (cause) {
      fail(cause);
    } finally {
      loading = false;
    }
  }

  function closeStream() {
    if (retry) clearTimeout(retry);
    retry = null;
    source?.close();
    source = null;
  }

  function openStream(id: string, startAfter = 0) {
    closeStream();
    let after = startAfter;
    const stream = new EventSource(eventsUrl(id, after));
    stream.onmessage = (message) => {
      try {
        const event = JSON.parse(message.data) as SessionEvent;
        if (event.sessionId !== id) return;
        const next = applyEvent(feed, event);
        feed = next;
        statuses = { ...statuses, [id]: next.status };
        after = Math.max(after, event.seq);
      } catch {
        // Ignore a malformed frame; the replay on reconnect recovers the log.
      }
    };
    stream.onerror = () => {
      stream.close();
      if (selected !== id) return;
      retry = setTimeout(() => openStream(id, after), 1000);
    };
    source = stream;
  }

  function select(id: string) {
    if (selected === id && view === 'session') {
      drawer = false;
      return;
    }
    selected = id;
    view = 'session';
    drawer = false;
    feed = emptyFeedState();
    statuses = { ...statuses, [id]: 'idle' };
    openStream(id);
  }

  async function createSession(input: {
    projectId?: string;
    root?: string;
    ponytailLevel: string;
    model?: string;
  }) {
    let projectId = input.projectId;
    if (input.root) {
      const name = input.root.split('/').filter(Boolean).at(-1) ?? 'project';
      const project = await api.createProject(name, input.root);
      projects = [...projects, project];
      projectId = project.id;
    }
    if (!projectId) throw new Error('Choose a Project or enter a path.');
    const created = await api.createSession(
      projectId,
      undefined,
      input.ponytailLevel,
      input.model
    );
    sessions = [created, ...sessions];
    select(created.id);
  }

  async function renameSession(session: Session) {
    const title = window.prompt('Rename session', session.title)?.trim();
    if (!title || title === session.title) return;
    try {
      const updated = await api.updateSession(session.id, title);
      sessions = sessions.map((item) => (item.id === session.id ? updated : item));
    } catch (cause) {
      fail(cause);
    }
  }

  async function deleteSession(session: Session) {
    if (!window.confirm(`Delete “${session.title}”? This cannot be undone.`)) return;
    try {
      await api.deleteSession(session.id);
      sessions = sessions.filter((item) => item.id !== session.id);
      if (selected === session.id) {
        closeStream();
        const next = sessions.find((item) => item.projectId === session.projectId) ?? sessions[0];
        selected = next?.id ?? null;
        feed = emptyFeedState();
        if (next) openStream(next.id);
      }
    } catch (cause) {
      fail(cause);
    }
  }

  async function sendPrompt(text: string, files: File[]) {
    if (!selected) throw new Error('Select a Session first.');
    if (working) {
      await api.steer(selected, text);
      return;
    }
    const images = await Promise.all(files.map(toPromptImage));
    await api.prompt(selected, text, images);
    feed = { ...feed, status: 'working' };
    statuses = { ...statuses, [selected]: 'working' };
  }

  async function cancel() {
    if (!selected) return;
    try {
      await api.cancel(selected);
      feed = { ...feed, status: 'idle' };
      statuses = { ...statuses, [selected]: 'idle' };
    } catch (cause) {
      fail(cause);
    }
  }

  async function saveSettings(next: SettingsDto) {
    const previous = settings;
    settings = next;
    settingsSaving = true;
    try {
      settings = await api.updateSettings(next);
    } catch (cause) {
      settings = previous;
      fail(cause);
    } finally {
      settingsSaving = false;
    }
  }
</script>

<div class="flex h-full">
  {#if drawer}
    <button
      class="fixed inset-0 z-20 bg-black/50 md:hidden"
      aria-label="Close sessions"
      onclick={() => (drawer = false)}
      transition:fadeAt
    ></button>
  {/if}

  <aside class="drawer w-[280px] shrink-0 {drawer ? 'open' : ''}">
    <Sidebar
      {projects}
      {sessions}
      {statuses}
      selectedId={view === 'session' ? selected : null}
      settingsActive={view === 'settings'}
      providersActive={view === 'providers'}
      onselect={select}
      onnew={() => (newOpen = true)}
      onrename={(session) => void renameSession(session)}
      ondelete={(session) => void deleteSession(session)}
      onsettings={() => {
        view = 'settings';
        drawer = false;
      }}
      onproviders={() => {
        view = 'providers';
        drawer = false;
      }}
    />
  </aside>

  <main class="flex min-w-0 flex-1 flex-col">
    <header class="flex items-center gap-3 border-b border-border px-4 py-2.5">
      <button class="btn md:hidden" aria-label="Open sessions" onclick={() => (drawer = true)}>☰</button>
      <div class="min-w-0 flex-1">
        <h1 class="truncate font-semibold tracking-[-0.01em]">
          {view === 'settings'
            ? 'Settings'
            : view === 'providers'
              ? 'Providers'
              : (current?.title ?? 'Emberloop')}
        </h1>
        <div class="truncate text-[12px] text-fg-3">
          {view === 'session' && projectOf ? `${projectOf.name} · ${projectOf.root}` : ''}
        </div>
      </div>
      {#if view === 'session' && current}
        <span class="chip hide-sm font-mono">{current.model}</span>
        <div
          class="hide-sm flex items-center gap-2 text-[12px] text-fg-3"
          title="Last request input tokens over the window"
        >
          <div class="meter"><i style={`width:${usagePct}%`}></i></div>
          <span class="tabular-nums">{usageLabel}</span>
        </div>
        <span class="flex items-center gap-1.5 text-[13px]">
          <span class={`dot ${statusDot[feed.status]} ${working ? 'pulse' : ''}`}></span>
          {statusWord[feed.status]}
        </span>
      {/if}
    </header>

    {#if error}
      <div
        class="flex items-center gap-3 border-b border-status-failed px-4 py-2 text-[13px]"
        style="background:#2a2222;color:#ffb3ad"
        transition:slideDown
      >
        <span class="flex-1">{error}</span>
        <button class="text-[12px] underline" onclick={() => (error = '')}>Dismiss</button>
      </div>
    {/if}

    {#if view === 'settings'}
      <Settings
        {settings}
        {models}
        saving={settingsSaving}
        onsave={(next) => void saveSettings(next)}
      />
    {:else if view === 'providers'}
      <Providers />
    {:else if loading}
      <div class="grid flex-1 place-items-center text-[13px] text-fg-3">Loading…</div>
    {:else if current}
      <Feed {feed} />
      <Composer
        working={working}
        onsubmit={sendPrompt}
        oncancel={cancel}
      />
    {:else}
      <div class="grid flex-1 place-items-center">
        <div class="flex flex-col items-center gap-3 text-center">
          <p class="text-[13px] text-fg-3">No Session selected. Create one to start.</p>
          <button class="btn btn-pri" onclick={() => (newOpen = true)}>New session</button>
        </div>
      </div>
    {/if}
  </main>
</div>

<NewSessionDialog
  bind:open={newOpen}
  {projects}
  {models}
  defaultLevel={settings.ponytail_default_level}
  oncreate={createSession}
/>
