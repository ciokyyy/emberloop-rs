<script lang="ts">
  import '../app.css';
  import { beforeNavigate, goto } from '$app/navigation';
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
  import { playAlert, registerWorker, reportVisible, unlockSound } from '$lib/notify';
  import SessionDialog from '$lib/components/SessionDialog.svelte';
  import CacheView from '$lib/components/CacheView.svelte';
  import Composer from '$lib/components/Composer.svelte';
  import Feed from '$lib/components/Feed.svelte';
  import DraftSession from '$lib/components/DraftSession.svelte';
  import NewSessionDialog from '$lib/components/NewSessionDialog.svelte';
  import PlanCard from '$lib/components/PlanCard.svelte';
  import Providers from '$lib/components/Providers.svelte';
  import Usage from '$lib/components/Usage.svelte';
  import Skills from '$lib/components/Skills.svelte';
  import SkillDetail from '$lib/components/SkillDetail.svelte';
  import Settings from '$lib/components/Settings.svelte';
  import Status from '$lib/components/Status.svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import WorkingIndicator from '$lib/components/WorkingIndicator.svelte';
  import FigcraftView from '$lib/components/FigcraftView.svelte';
  import ProjectDrawer from '$lib/components/ProjectDrawer.svelte';
  import SessionMenu from '$lib/components/SessionMenu.svelte';
  import { PanelsTopLeft, RotateCw } from '@lucide/svelte';
  import { applyEvent, applySnapshot, emptyFeedState, type FeedState } from '$lib/feed';
  import { chosenWindow } from '$lib/contextMeter';
  import { toPromptImage } from '$lib/images';
  import { modelIconId } from '$lib/brands';
  import { cachePath, designPath, newPath, newSessionProjects, sessionPath, skillPath, viewOf, viewPath, type Page, type View } from '$lib/route';

  let { children } = $props();
  /** The Skill page's SKILL.md path, which its component loads and the header shows. */
  let skillPathText = $state('');
  let projects = $state<Project[]>([]);
  let sessions = $state<Session[]>([]);
  let models = $state<Model[]>([]);
  let detailsOpen = $state(false);
  let detailsTab = $state<'model' | 'context'>('model');
  let settings = $state<SettingsDto>({
    title_generation: true,
    title_model: 'openrouter/openrouter/free',
    default_model: 'gemini-3.8-flash',
    spec_reviewer: [{ model: 'gemini-3.8-flash', reasoning_effort: null }],
    standards_reviewer: [{ model: 'gemini-3.8-flash', reasoning_effort: null }],
    subagent_tiers: {
      smol: [{ model: 'gemini-3.8-flash', reasoning_effort: null }],
      mid: [{ model: 'gemini-3.8-flash', reasoning_effort: null }],
      top: [{ model: 'gemini-3.8-flash', reasoning_effort: null }]
    },
    proxy_compression: true,
    ponytail_default_level: 'full',
    todo_reminders: true,
    todo_reminders_max: 3,
    todo_auto_expand: true,
    todo_hide_when_done: true,
    max_context_limit: 20,
    min_pressure_benefit_tokens: 0,
    emergency_threshold_percent: 25,
    preserve_recent_messages: 3,
    protected_tools: [],
    inject_tool: true,
    inject_nudge: true,
    tier2_trigger: 1000,
    tier3_trigger: 2000,
    skills_review_interval: 5
  });
  // The window of events in hand: the tail on open, one page older per click. `feed` is folded from
  // it, so a page prepended at the front rebuilds through the same reducer as the live tail.
  let log = $state<SessionEvent[]>([]);
  let feed = $state<FeedState>(emptyFeedState());
  let loadingMore = $state(false);
  let historyExhausted = $state(false);
  let statuses = $state<Record<string, SessionStatus>>({});
  let loading = $state(true);
  let error = $state('');
  let newOpen = $state(false);
  // The dialog asks for a Session by default; the "New project" button asks for
  // a path instead.
  let newProject = $state(false);
  // The draft: the model picked before a Session exists. `draftProject` remembers which
  // Project's draft already took the defaults, so a model pick does not reset the draft.
  let draftModel = $state('');
  let draftProject: string | null = null;
  let drawer = $state(false);
  let projectDrawerOpen = $state(false);
  let settingsSaving = $state(false);
  // The turn-finished or error line the page heard while it was open; the daemon stays quiet then.
  let notice = $state<{ title: string; failed: boolean; sessionId: string } | null>(null);

  let source: EventSource | null = null;
  let retry: ReturnType<typeof setTimeout> | null = null;
  // The pending SSE batch and its animation frame, so a replay folds into one update per frame.
  let frame: number | null = null;
  let flush: (() => void) | null = null;
  let sendChain: Promise<void> = Promise.resolve();
  let inFlightSends = 0;
  let optSeq = 0;
  const PAGE = 8;
  // browser's back button all land on the same screen.
  const view = $derived<View>(viewOf(page.route.id));
  const selected = $derived(page.params.session ?? null);
  // A board opened from a Session keeps that Session as the agent it drives.
  const designSession = $derived(view === 'design' ? page.url.searchParams.get('session') : null);
  const hasMore = $derived(!historyExhausted && (log[0]?.seq ?? 1) > 1);
  const current = $derived(sessions.find((session) => session.id === selected) ?? null);
  // The URL's Project scopes the sidebar: one Project and its Sessions.
  const scope = $derived(page.params.project ?? null);
  const listed = $derived(newSessionProjects(scope, projects));

  // Upstream owns its WASM runner; a full navigation releases its GPU and input handlers.
  beforeNavigate((navigation) => {
    if (view === 'design' && navigation.to && !navigation.willUnload && navigation.to.url.pathname !== page.url.pathname) {
      navigation.cancel();
      window.location.assign(navigation.to.url.href);
    }
  });
  const projectOf = $derived(projects.find((project) => project.id === current?.projectId) ?? null);
  const scopeProject = $derived(projects.find((project) => project.id === scope) ?? null);
  const activeProject = $derived(scopeProject ?? projectOf ?? projects[0] ?? null);
  const draftOf = $derived(models.find((model) => model.id === draftModel) ?? null);
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
    void registerWorker();
    // A focused page plays its own alert; the daemon needs to know it is here, not gone.
    const gesture = () => unlockSound();
    window.addEventListener('pointerdown', gesture, { once: true });
    window.addEventListener('keydown', gesture, { once: true });
    return () => {
      closeStream();
      window.removeEventListener('pointerdown', gesture);
      window.removeEventListener('keydown', gesture);
    };
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
    // A reconnect timer can outlive the Session it was opened for; reopening then would close the
    // current Session's stream, whose events the guard below drops, and the feed stays blank
    // until a reload.
    if (selected !== id) return;
    closeStream();
    let after = startAfter;
    // A replay carries the Session's whole tail window; only an event that lands after the page
    // opened is news, or opening a finished Session would alert every time.
    const openedAt = Date.now();
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
      if (batch.some((event) => event.kind === 'notice' && event.data.action === 'subagent_created')) {
        void api.listSessions().then((updated) => { sessions = updated; }).catch((cause) => { error = String(cause); });
      }
      log = [...log, ...batch];
      const finished = batch.find(
        (event) => event.at >= openedAt && (event.kind === 'error' || (event.kind === 'status' && event.data.state === 'idle'))
      );
      if (finished && document.visibilityState === 'visible') {
        const failed = finished.kind === 'error';
        notice = { title: failed ? 'The turn failed.' : 'The turn finished.', failed, sessionId: id };
        playAlert(failed);
      }
      feed = batch.reduce(applyEvent, feed);
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
      if (page.length === 0) {
        historyExhausted = true;
        return;
      }
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
  function openView(next: Page) {
    drawer = false;
    void goto(viewPath(scope ?? projectOf?.id ?? projects[0]?.id ?? null, next));
  }

  // The cache page is one level under the Session: its details dialog closes as the URL changes.
  function openCache() {
    if (!current) return;
    detailsOpen = false;
    void goto(cachePath(current.projectId, current.id));
  }
  // The daemon pushes only for a Session no open page is showing, so the page reports its own.
  $effect(() => {
    reportVisible(selected);
    return () => reportVisible(null);
  });

  // The stream follows the URL: a new Session id tears the old stream down and starts a new one.
  $effect(() => {
    const id = selected;
    log = [];
    historyExhausted = false;
    feed = emptyFeedState();
    if (!id) {
      closeStream();
      return;
    }
    openStream(id);
    return () => closeStream();
  });

  // New Session opens the draft page: no Session exists until the first prompt. Without a
  // Project there is nowhere to hang the draft, so the dialog asks for one by path.
  function startSession() {
    const projectId = scope ?? projects[0]?.id ?? null;
    if (projectId) return void goto(newPath(projectId));
    newProject = true;
    newOpen = true;
  }

  // A draft takes the configured defaults once per Project.
  $effect(() => {
    if (view !== 'new' || !models.length || draftProject === scope) return;
    draftProject = scope;
    draftModel = models.some((entry) => entry.id === settings.default_model) ? settings.default_model : models[0].id;
  });

  // The draft's first prompt creates the Session, then sends: the URL moves to the real one.
  async function sendDraft(text: string, files: File[], thoughtLevel: string) {
    const projectId = scope ?? projects[0]?.id;
    if (!projectId) throw new Error('Choose a Project first.');
    if (!draftModel) throw new Error('Choose a model first.');
    const images = await Promise.all(files.map(toPromptImage));
    const created = await api.createSession(projectId, undefined, draftModel, thoughtLevel || undefined);
    sessions = [created, ...sessions];
    await api.prompt(created.id, text, images);
    // The draft's text is spent; the new Session keeps its own.
    try { localStorage.removeItem(`emberloop-rs:draft:draft:${projectId}`); } catch { /* no storage */ }
    await goto(sessionPath(projectId, created.id));
  }

  async function createSession(input: {
    projectId?: string;
    root?: string;
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
    await goto(newPath(projectId));
  }

  async function cloneProject(repo: string, location: string) {
    const project = await api.cloneProject(repo, location);
    projects = [...projects, project];
    await goto(newPath(project.id));
  }

  // Switching Project lands on its most recent Session; with none, on its Settings, since a
  // Session URL needs an id.
  function selectProject(id: string) {
    const latest = sessions
      .filter((session) => session.projectId === id)
      .sort((a, b) => b.updatedAt - a.updatedAt)[0];
    // Switching Project leaves a Skill's page: its list is the Project-scoped view.
    const page = view === 'session' || view === 'new' || view === 'cache' ? 'settings' : view === 'skill' ? 'skills' : view;
    void goto(latest ? sessionPath(id, latest.id) : viewPath(id, page));
  }
  async function renameSession(session: Session, title: string) {
    if (title === session.title) return;
    try {
      const updated = await api.updateSession(session.id, { title });
      sessions = sessions.map((item) => (item.id === session.id ? updated : item));
    } catch (cause) {
      fail(cause);
    }
  }

  async function deleteSession(session: Session) {
    // The Sidebar's confirm dialog asks first.
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
    const targetSession = selected;
    if (!targetSession) throw new Error('Select a Session first.');
    const isSteer = working || inFlightSends > 0;
    const optId = `opt-${++optSeq}`;
    const imagesCount = files.length || undefined;
    feed = {
      ...feed,
      status: isSteer ? feed.status : 'working',
      items: [
        ...feed.items,
        isSteer
          ? { id: optId, kind: 'steer', text, imagesCount, route: 'pending', optimistic: true }
          : { id: optId, kind: 'user', text, imagesCount, optimistic: true }
      ]
    };
    if (!isSteer) statuses = { ...statuses, [targetSession]: 'working' };
    inFlightSends += 1;
    const job = sendChain
      .catch(() => {})
      .then(async () => {
        const images = await Promise.all(files.map(toPromptImage));
        if (isSteer) await api.steer(targetSession, text, images);
        else await api.prompt(targetSession, text, images);
      });
    sendChain = job;
    try {
      await job;
    } catch (cause) {
      if (selected === targetSession) {
        const hadOtherWork = working && inFlightSends > 1;
        feed = {
          ...feed,
          items: feed.items.filter((item) => item.id !== optId),
          status: !isSteer && !hadOtherWork ? 'idle' : feed.status
        };
        if (!isSteer && !hadOtherWork) statuses = { ...statuses, [targetSession]: 'idle' };
      }
      throw cause;
    } finally {
      inFlightSends = Math.max(0, inFlightSends - 1);
    }
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

<svelte:window onkeydown={(event) => { if (drawer && event.key === 'Escape') drawer = false; }} />

<div class="flex h-full">
  {#if drawer}
    <button
      class="drawer-scrim {view === 'design' ? '' : 'md:hidden'}"
      aria-label="Close sessions"
      onclick={() => (drawer = false)}
      transition:fadeAt
    ></button>
  {/if}

  {#if view !== 'design' || drawer}
    <aside
      class="drawer w-[var(--drawer-width)] shrink-0 {drawer ? 'open' : ''} {view === 'design'
        ? 'drawer-floating'
        : ''}"
    >
    <Sidebar
      {projects}
      projectId={scope}
      {sessions}
      {models}
      {statuses}
      selectedId={view === 'session' || view === 'cache' ? selected : null}
      draftModel={view === 'new' && scope ? draftModel : null}
      ondraft={() => { if (scope) void goto(newPath(scope)); }}
      settingsActive={view === 'settings'}
      statusActive={view === 'status'}
      providersActive={view === 'providers'}
      usageActive={view === 'usage'}
      skillsActive={view === 'skills' || view === 'skill'}
      onselect={select}
      onselectproject={selectProject}
      onnew={startSession}
      onnewproject={() => {
        newProject = true;
        newOpen = true;
      }}
      onrename={(session, title) => void renameSession(session, title)}
      ondelete={(session) => void deleteSession(session)}
      onsettings={() => openView('settings')}
      onstatus={() => openView('status')}
      onproviders={() => openView('providers')}
      onusage={() => openView('usage')}
      onskills={() => openView('skills')}
    />
    </aside>
  {/if}

  <main class="flex min-w-0 flex-1 flex-col">
    <header class="flex h-14 shrink-0 items-center gap-3 border-b border-border px-4">
      <button class="btn {view === 'design' ? '' : 'md:hidden'}" aria-label="Open sessions" onclick={() => (drawer = true)}>☰</button>
      <SessionMenu session={current} project={projectOf}>
        <h1 class="truncate text-[14px] font-semibold tracking-[-0.01em] text-fg-1">
          {view === 'new'
            ? 'New session'
            : view === 'settings'
              ? 'Settings'
            : view === 'status'
              ? 'Status'
            : view === 'providers'
              ? 'Providers'
              : view === 'skill'
                ? decodeURIComponent(page.params.name ?? 'Skills')
              : view === 'skills'
                ? 'Skills'
              : view === 'usage'
                ? 'Usage'
              : view === 'design'
                ? 'Design Canvas · FigCraft'
                : (current?.title ?? 'Emberloop')}
        </h1>
        <div class="truncate text-[12px] text-fg-3">
          {view === 'new'
            ? scopeProject ? `${scopeProject.name} · ${scopeProject.root}` : ''
            : view === 'skill'
              ? skillPathText
            : view === 'design' && activeProject
              ? `${activeProject.name} · ${activeProject.root}`
              : view === 'session' && projectOf ? `${projectOf.name} · ${projectOf.root}` : ''}
        </div>
      </SessionMenu>

      <button
        type="button"
        class="grid size-8 shrink-0 place-items-center rounded-xl border border-border bg-surface-1 text-fg-2 hover:bg-surface-3 hover:text-fg-1"
        aria-label="Reload app"
        title="Reload"
        onclick={() => window.location.reload()}
      >
        <RotateCw class="size-4" />
      </button>
      {#if activeProject}
        <button
          class="grid size-8 shrink-0 place-items-center rounded-xl border border-border bg-surface-1 text-fg-2 hover:bg-surface-3 hover:text-fg-1"
          aria-label="Project tools"
          title="Project tools"
          onclick={() => (projectDrawerOpen = true)}
        >
          <PanelsTopLeft class="size-4" />
        </button>
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

    {#if notice}
      <div
        class="flex items-center gap-3 border-b px-4 py-2 text-[13px]"
        class:border-status-failed={notice.failed}
        class:border-border={!notice.failed}
        style={notice.failed ? 'background:#2a2222;color:#ffb3ad' : 'background:#20221f;color:#dfe3d8'}
        transition:slideDown
      >
        <span class="flex-1">{notice.title}</span>
        {#if notice.sessionId !== selected}
          <button class="text-[12px] underline" onclick={() => select(notice?.sessionId ?? '')}>Open</button>
        {/if}
        <button class="text-[12px] underline" onclick={() => (notice = null)}>Dismiss</button>
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
    {:else if view === 'skills'}
      <Skills projectId={activeProject?.id ?? null}
        onopen={(name) => { if (activeProject) void goto(skillPath(activeProject.id, name)); }} />
    {:else if view === 'skill' && activeProject}
      <SkillDetail name={decodeURIComponent(page.params.name ?? '')} bind:path={skillPathText}
        onback={() => openView('skills')} />
    {:else if view === 'usage'}
      <Usage />
    {:else if view === 'status'}
      <Status />
    {:else if view === 'design' && activeProject}
      <FigcraftView
        project={activeProject}
        {sessions}
        currentSessionId={designSession}
        onbacktofeed={(sid) => window.location.assign(sessionPath(activeProject.id, sid))}
      />
    {:else if loading}
      <div class="grid flex-1 place-items-center text-[13px] text-fg-3">Loading…</div>
    {:else if view === 'cache' && current}
      <CacheView sessionId={current.id} status={feed.status} />
    {:else if view === 'new'}
      {#key scope}
        <DraftSession sessionId={`draft:${scope ?? ''}`} {models}
          bind:model={draftModel} onsubmit={sendDraft} />
      {/key}
    {:else if current}
      <Feed {feed} projectId={current.projectId} sessionId={current.id} {hasMore} {loadingMore} onloadmore={loadEarlier} />
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
          providerIcon={currentModel ? modelIconId(currentModel) : ''}
          imageInput={currentModel?.image_input ?? false}
          ondetails={(tab) => { detailsTab = tab; detailsOpen = true; }}
          onsubmit={sendPrompt}
          oncancel={cancel}
        />
        <SessionDialog bind:open={detailsOpen} bind:tab={detailsTab} {models}
          model={current.model} thoughtLevel={current.thoughtLevel ?? ''}
          used={feed.usage?.used ?? null} size={contextSize} breakdown={feed.usage?.breakdown ?? null}
          disabled={working} onapply={applyModel} oncache={openCache} sessionId={selected ?? ''} />
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
  defaultModel={settings.default_model}
  oncreate={createSession}
  onclone={cloneProject}
/>

{#if activeProject}
  <ProjectDrawer
    project={activeProject}
    open={projectDrawerOpen}
    onclose={() => (projectDrawerOpen = false)}
    onopendesign={() => {
      // The design canvas owns its own window: the installed app has no URL bar to come back with.
      window.open(designPath(activeProject.id, selected ?? designSession), '_blank', 'noopener');
    }}
  />
{/if}
