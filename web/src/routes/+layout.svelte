<script lang="ts">
  import '../app.css';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
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
  import SessionDialog from '$lib/components/SessionDialog.svelte';
  import Composer from '$lib/components/Composer.svelte';
  import Feed from '$lib/components/Feed.svelte';
  import NewSessionDialog from '$lib/components/NewSessionDialog.svelte';
  import PlanCard from '$lib/components/PlanCard.svelte';
  import Providers from '$lib/components/Providers.svelte';
  import Usage from '$lib/components/Usage.svelte';
  import Settings from '$lib/components/Settings.svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import WorkingIndicator from '$lib/components/WorkingIndicator.svelte';
  import { applyEvent, applySnapshot, emptyFeedState, type FeedState } from '$lib/feed';
  import { chosenWindow } from '$lib/contextMeter';
  import { toPromptImage } from '$lib/images';
  import { newSessionProjects, sessionPath, viewOf, viewPath, type View } from '$lib/route';

  let { children } = $props();
  let projects = $state<Project[]>([]);
  let sessions = $state<Session[]>([]);
  let models = $state<Model[]>([]);
  let detailsOpen = $state(false);
  let detailsTab = $state<'model' | 'context'>('model');
  let settings = $state<SettingsDto>({
    title_generation: true,
    title_model: 'openrouter/openrouter/free',
    proxy_compression: true,
    ponytail_default_level: 'full',
    todo_reminders: true,
    todo_reminders_max: 3,
    todo_auto_expand: true,
    todo_hide_when_done: true,
    max_context_limit: 75,
    min_pressure_benefit_tokens: 0,
    emergency_threshold_percent: 95,
    preserve_recent_messages: 3,
    protected_tools: [],
    inject_tool: true,
    inject_nudge: true,
    tier2_trigger: 1000,
    tier3_trigger: 2000
  });
  // The window of events in hand: the tail on open, one page older per click. `feed` is folded from
  // it, so a page prepended at the front rebuilds through the same reducer as the live tail.
  let log = $state<SessionEvent[]>([]);
  let feed = $state<FeedState>(emptyFeedState());
  let loadingMore = $state(false);
  let statuses = $state<Record<string, SessionStatus>>({});
  let loading = $state(true);
  let error = $state('');
  let newOpen = $state(false);
  // The dialog asks for a Session by default; the "New project" button asks for
  // a path instead.
  let newProject = $state(false);
  let drawer = $state(false);
  let settingsSaving = $state(false);

  let source: EventSource | null = null;
  let retry: ReturnType<typeof setTimeout> | null = null;
  // The pending SSE batch and its animation frame, so a replay folds into one update per frame.
  let frame: number | null = null;
  let flush: (() => void) | null = null;
  const PAGE = 8;
  // The URL owns which Session and which view are showing, so a refresh, a pasted link and the
  // browser's back button all land on the same screen.
  const view = $derived<View>(viewOf(page.route.id));
  const selected = $derived(page.params.session ?? null);
  const hasMore = $derived((log[0]?.seq ?? 1) > 1);
  const current = $derived(sessions.find((session) => session.id === selected) ?? null);
  // The URL's Project scopes the sidebar: one Project and its Sessions.
  const scope = $derived(page.params.project ?? null);
  const listed = $derived(newSessionProjects(scope, projects));
  const projectOf = $derived(projects.find((project) => project.id === current?.projectId) ?? null);
  const working = $derived(feed.status === 'working');
  // activeTool names the call the trailing run of the feed is making right now.
  const activeTool = $derived.by(() => {
    const tail = feed.items.at(-1);
    if (tail?.kind === 'thinking') return 'thinking';
    if (tail?.kind !== 'tool' || tail.finished) return '';
    return tail.tool || tail.name || '';
  });
  const currentModel = $derived(models.find((model) => model.id === current?.model) ?? null);
  const contextSize = $derived(chosenWindow(currentModel?.context_window, feed.usage?.window));


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
      // A bare `/` or a link to a Session that is gone lands on the most recent one.
      const fallback = [...loadedSessions].sort((a, b) => b.updatedAt - a.updatedAt)[0];
      const wanted = page.params.session;
      const known = loadedSessions.some((session) => session.id === wanted);
      if (fallback && view === 'session' && !known) {
        await goto(sessionPath(fallback.projectId, fallback.id), { replaceState: true });
      }
    } catch (cause) {
      fail(cause);
    } finally {
      loading = false;
    }
  }

  function closeStream() {
    if (retry) clearTimeout(retry);
    retry = null;
    if (frame !== null) cancelAnimationFrame(frame);
    frame = null;
    flush?.();
    flush = null;
    source?.close();
    source = null;
  }

  function openStream(id: string, startAfter = 0) {
    closeStream();
    let after = startAfter;
    // A replay arrives as hundreds of frames in one network task. Fold them into one update per
    // frame instead of one per event, so opening a long Session does not stall the main thread.
    let pending: SessionEvent[] = [];
    const apply = () => {
      frame = null;
      flush = null;
      if (selected !== id || pending.length === 0) {
        pending = [];
        return;
      }
      const batch = pending;
      pending = [];
      for (const event of batch) {
        if (event.kind === 'session_title' && typeof event.data.title === 'string') {
          sessions = sessions.map((session) => session.id === id ? { ...session, title: event.data.title as string } : session);
        }
      }
      log = [...log, ...batch];
      feed = batch.reduce(applyEvent, feed);
      statuses = { ...statuses, [id]: feed.status };
    };
    const schedule = () => {
      if (flush) return;
      flush = () => {
        if (frame !== null) cancelAnimationFrame(frame);
        apply();
      };
      frame = requestAnimationFrame(flush);
    };
    // First open asks for the tail window only; a reconnect resumes from its own seq.
    const stream = new EventSource(eventsUrl(id, after, after === 0 ? PAGE : 0));
    stream.onmessage = (message) => {
      try {
        const event = JSON.parse(message.data) as SessionEvent;
        if (event.sessionId !== id) return;
        pending.push(event);
        after = Math.max(after, event.seq);
        schedule();
      } catch {
        // Ignore a malformed frame; the replay on reconnect recovers the log.
      }
    };
    // Sent once, after the tail window, on a first load: the current status and context usage.
    stream.addEventListener('snapshot', (message) => {
      apply();
      if (selected !== id) return;
      try {
        const next = applySnapshot(feed, JSON.parse((message as MessageEvent).data));
        feed = next;
        statuses = { ...statuses, [id]: next.status };
      } catch {
        // A malformed snapshot leaves the reducer's own view in place.
      }
    });
    stream.onerror = () => {
      stream.close();
      if (selected !== id) return;
      retry = setTimeout(() => openStream(id, after), 1000);
    };
    source = stream;
  }

  // loadEarlier walks one page back from the oldest event in hand and rebuilds the feed from the
  // window, so the rows behind a click are folded by the same reducer as the live ones.
  async function loadEarlier() {
    const id = selected;
    if (!id || loadingMore || !hasMore) return;
    loadingMore = true;
    try {
      const page = await api.history(id, log[0]?.seq ?? 0, PAGE);
      if (selected !== id) return;
      log = [...page, ...log];
      // An older page changes the rows, never the Session's current status or usage.
      feed = { ...log.reduce(applyEvent, emptyFeedState()), status: feed.status, usage: feed.usage };
    } catch (cause) {
      fail(cause);
    } finally {
      loadingMore = false;
    }
  }

  function select(id: string) {
    drawer = false;
    const pick = sessions.find((session) => session.id === id);
    if (pick) void goto(sessionPath(pick.projectId, id));
  }

  // openView keeps Settings and Providers on a Project's URL, so Back returns to its Session.
  function openView(next: Exclude<View, 'session'>) {
    drawer = false;
    void goto(viewPath(projectOf?.id ?? projects[0]?.id ?? null, next));
  }

  // The stream follows the URL: a new Session id tears the old stream down and starts a new one.
  $effect(() => {
    const id = selected;
    log = [];
    feed = emptyFeedState();
    if (!id) {
      closeStream();
      return;
    }
    openStream(id);
    return () => closeStream();
  });

  // A Project URL only preselects the Project. The dialog still opens, because the
  // model is a choice the create request needs.
  function startSession() {
    newProject = false;
    newOpen = true;
  }

  async function createSession(input: {
    projectId?: string;
    root?: string;
    ponytailLevel: string;
    model?: string;
    thoughtLevel?: string;
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
      input.model,
      input.thoughtLevel
    );
    sessions = [created, ...sessions];
    await goto(sessionPath(projectId, created.id));
  }

  async function renameSession(session: Session) {
    const title = window.prompt('Rename session', session.title)?.trim();
    if (!title || title === session.title) return;
    try {
      const updated = await api.updateSession(session.id, { title });
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
        const next = sessions.find((item) => item.projectId === session.projectId) ?? sessions[0];
        await goto(next ? sessionPath(next.projectId, next.id) : '/');
      }
    } catch (cause) {
      fail(cause);
    }
  }

  async function sendPrompt(text: string, files: File[]) {
    if (!selected) throw new Error('Select a Session first.');
    if (working) {
      if (files.length) throw new Error('Images cannot be sent while the Session is working.');
      await api.steer(selected, text);
      return;
    }
    const images = await Promise.all(files.map(toPromptImage));
    await api.prompt(selected, text, images);
    feed = { ...feed, status: 'working' };
    statuses = { ...statuses, [selected]: 'working' };
  }

  async function applyModel(input: { model: string; thoughtLevel: string }) {
    if (!selected) throw new Error('Select a Session first.');
    const updated = await api.updateSession(selected, input);
    sessions = sessions.map((item) => item.id === updated.id ? updated : item);
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
      projects={listed}
      {sessions}
      {models}
      {statuses}
      selectedId={view === 'session' ? selected : null}
      settingsActive={view === 'settings'}
      providersActive={view === 'providers'}
      usageActive={view === 'usage'}
      onselect={select}
      onnew={startSession}
      onnewproject={
        scope
          ? () => {
              newProject = true;
              newOpen = true;
            }
          : undefined
      }
      onrename={(session) => void renameSession(session)}
      ondelete={(session) => void deleteSession(session)}
      onsettings={() => openView('settings')}
      onproviders={() => openView('providers')}
      onusage={() => openView('usage')}
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
              : view === 'usage'
                ? 'Usage'
                : (current?.title ?? 'Emberloop')}
        </h1>
        <div class="truncate text-[12px] text-fg-3">
          {view === 'session' && projectOf ? `${projectOf.name} · ${projectOf.root}` : ''}
        </div>
      </div>

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
      <Providers onmodels={(list) => (models = list)} />
    {:else if view === 'usage'}
      <Usage />
    {:else if loading}
      <div class="grid flex-1 place-items-center text-[13px] text-fg-3">Loading…</div>
    {:else if current}
      <Feed {feed} {hasMore} {loadingMore} onloadmore={loadEarlier} />
      <!-- The plan card and the turn line sit in the composer's own column, right above it. -->
      <div class="px-4">
        <div class="mx-auto max-w-[760px] pb-1">
          <PlanCard
            items={feed.todos}
            autoExpand={settings.todo_auto_expand}
            hideWhenDone={settings.todo_hide_when_done}
          />
          <WorkingIndicator status={feed.status} tool={activeTool} class="pb-1" />
        </div>
      </div>
      {#key selected}
        <Composer
          sessionId={selected ?? ''}
          working={working}
          contextUsed={feed.usage?.used ?? null}
          {contextSize}
          model={currentModel?.label ?? current.model}
          imageInput={currentModel?.image_input ?? false}
          thoughtLevel={current.thoughtLevel ?? ''}
          ondetails={(tab) => { detailsTab = tab; detailsOpen = true; }}
          onsubmit={sendPrompt}
          oncancel={cancel}
        />
        <SessionDialog bind:open={detailsOpen} bind:tab={detailsTab} {models}
          model={current.model} thoughtLevel={current.thoughtLevel ?? ''}
          used={feed.usage?.used ?? null} size={contextSize} breakdown={feed.usage?.breakdown ?? null}
          disabled={working} onapply={applyModel} />
      {/key}
    {:else}
      <div class="grid flex-1 place-items-center">
        <div class="flex flex-col items-center gap-3 text-center">
          <p class="text-[13px] text-fg-3">No Session selected. Create one to start.</p>
          <button class="btn btn-pri" onclick={startSession}>New session</button>
        </div>
      </div>
    {/if}
  </main>
</div>

{@render children()}

<NewSessionDialog
  bind:open={newOpen}
  projects={listed}
  newProject={newProject}
  {models}
  defaultLevel={settings.ponytail_default_level}
  oncreate={createSession}
/>
