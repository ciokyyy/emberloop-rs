<script lang="ts">
  import { ArrowUp, MessageSquare, Bot } from '@lucide/svelte';
  import type { Project, Session, SessionEvent } from '../api/types';
  import { eventsUrl } from '../api';
  import { applyEvent, applySnapshot, emptyFeedState } from '../feed';
  import Button from './ui/button.svelte';
  import Input from './ui/input/input.svelte';
  import * as Select from './ui/select';

  let { project, sessions, currentSessionId = null, onsendprompt, onbacktofeed }: {
    project: Project;
    sessions: Session[];
    currentSessionId?: string | null;
    onsendprompt: (sessionId: string, prompt: string) => Promise<void>;
    onbacktofeed?: (sessionId: string) => void;
  } = $props();
  let promptText = $state('');
  let sending = $state(false);
  let error = $state('');
  let selectedSessionId = $state('');
  let feed = $state(emptyFeedState());
  let connected = $state(false);
  const projectSessions = $derived(sessions.filter((s) => s.projectId === project.id));
  const activeSession = $derived(projectSessions.find((s) => s.id === selectedSessionId) ?? null);

  $effect(() => {
    if (!projectSessions.some((s) => s.id === selectedSessionId)) {
      selectedSessionId = projectSessions.find((s) => s.id === currentSessionId)?.id ?? projectSessions[0]?.id ?? '';
    }
  });

  $effect(() => {
    const id = activeSession?.id;
    feed = emptyFeedState();
    connected = false;
    if (!id || typeof EventSource === 'undefined') return;
    const stream = new EventSource(eventsUrl(id, 0, 1));
    stream.onopen = () => { connected = true; };
    stream.onerror = () => { connected = false; };
    stream.onmessage = (message) => {
      try {
        const event: SessionEvent = JSON.parse(message.data);
        if (event.sessionId === id && event.kind === 'status') feed = applyEvent(feed, event);
      } catch { connected = false; }
    };
    stream.addEventListener('snapshot', (message) => {
      try { feed = applySnapshot(feed, JSON.parse((message as MessageEvent).data)); }
      catch { connected = false; }
    });
    return () => stream.close();
  });

  async function send() {
    const text = promptText.trim();
    if (!text || !activeSession || sending) return;
    const sessionId = activeSession.id;
    sending = true;
    error = '';
    try {
      await onsendprompt(sessionId, text);
      promptText = '';
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally { sending = false; }
  }
</script>

<div class="pointer-events-none absolute inset-x-0 bottom-6 z-40 flex flex-col items-center gap-2 px-4">
  {#if error}<p role="alert" class="pointer-events-auto rounded-lg border border-border bg-bg-1 px-4 py-2 text-sm text-fg">{error}</p>{/if}
  <div class="pointer-events-auto flex w-full max-w-2xl items-center gap-3 rounded-full border border-border bg-bg-1 px-4 py-2 shadow-xl">
    <Bot class="size-4 shrink-0 text-fg-2" />
    {#if projectSessions.length}
      <Select.Root type="single" bind:value={selectedSessionId} disabled={sending}>
        <Select.Trigger aria-label="Agent session" class="max-w-44 border-0 bg-bg-2 text-xs">
          {activeSession?.title || 'Choose session'}
        </Select.Trigger>
        <Select.Content>
          {#each projectSessions as session (session.id)}
            <Select.Item value={session.id} label={session.title}>{session.title || 'Untitled session'}</Select.Item>
          {/each}
        </Select.Content>
      </Select.Root>
    {/if}
    <span class="text-xs text-fg-3" role="status">{!activeSession ? 'No sessions' : !connected ? 'Connecting' : feed.status}</span>
    <form class="flex min-w-0 flex-1 items-center gap-2" onsubmit={(event) => { event.preventDefault(); void send(); }}>
      <Input aria-label="Agent prompt" placeholder={activeSession ? 'Prompt agent…' : 'Create a session in chat first'} bind:value={promptText} disabled={!activeSession || sending} class="h-8 border-0 bg-transparent text-xs shadow-none" />
      <Button type="submit" aria-label="Send to agent" disabled={!activeSession || !promptText.trim() || sending} class="rounded-full"><ArrowUp class="size-4" /></Button>
    </form>
    {#if activeSession && onbacktofeed}
      <Button aria-label="Open chat" onclick={() => activeSession && onbacktofeed?.(activeSession.id)}><MessageSquare class="size-4" /></Button>
    {/if}
  </div>
</div>
