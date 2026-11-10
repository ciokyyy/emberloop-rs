<script lang="ts">
  import { tick } from 'svelte';
  import { ArrowUp, MessageSquare, Bot } from '@lucide/svelte';
  import type { Project, Session, SessionEvent } from '../api/types';
  import { eventsUrl } from '../api';
  import { applyEvent, applySnapshot, emptyFeedState } from '../feed';

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
  let feed = $state(emptyFeedState());
  let connected = $state(false);
  // The session the board was opened from is the agent you control, so there is no switcher.
  const activeSession = $derived(
    sessions.find((session) => session.id === currentSessionId && session.projectId === project.id) ?? null
  );

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

  // Collapsed pill parked bottom-right so it never covers the editor's bottom toolbar; drag to move.
  let open = $state(false);
  let offset = $state({ x: 0, y: 0 });
  let root = $state<HTMLElement | null>(null);
  let dragged = false;
  let drag: { x: number; y: number; ox: number; oy: number; box: DOMRect } | null = null;

  // Keeps the pill on screen: reopening changes its box, and a resized window moves the edges.
  function clamp() {
    // The phone sheet is anchored full width, so an 8px margin would push its right edge off screen.
    if (open && window.innerWidth < 640) return;
    const box = root?.getBoundingClientRect();
    if (!box) return;
    const x = Math.max(-box.left + 8, Math.min(offset.x, window.innerWidth - box.right - 8));
    const y = Math.max(-box.top + 8, Math.min(offset.y, window.innerHeight - box.bottom - 8));
    if (x !== offset.x || y !== offset.y) offset = { x, y };
  }

  function grab(event: PointerEvent) {
    if (!root) return;
    dragged = false;
    drag = { x: event.clientX, y: event.clientY, ox: offset.x, oy: offset.y, box: root.getBoundingClientRect() };
    // After the drag state: a refused capture must not kill the gesture.
    (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
  }

  function move(event: PointerEvent) {
    if (!drag) return;
    if (Math.abs(event.clientX - drag.x) > 4 || Math.abs(event.clientY - drag.y) > 4) dragged = true;
    const x = drag.ox + event.clientX - drag.x;
    const y = drag.oy + event.clientY - drag.y;
    offset = {
      x: Math.max(-drag.box.left + 8, Math.min(x, window.innerWidth - drag.box.right - 8)),
      y: Math.max(-drag.box.top + 8, Math.min(y, window.innerHeight - drag.box.bottom - 8))
    };
  }

  async function toggle() {
    if (dragged) { dragged = false; return; }
    open = !open;
    // The phone sheet is full width and bottom anchored, so a dragged offset would push it off screen.
    if (window.innerWidth < 640) offset = { x: 0, y: 0 };
    await tick();
    clamp();
  }
</script>
<svelte:window onresize={clamp} />
<div
  bind:this={root}
  class="absolute bottom-3 right-3 z-40 flex max-w-[calc(100%-1.5rem)] flex-col items-end gap-2 sm:bottom-6 {open
    ? 'max-sm:inset-x-0 max-sm:bottom-0 max-sm:max-w-none'
    : ''}"
  style:transform="translate({offset.x}px, {offset.y}px)"
>
  {#if error}<p role="alert" class="rounded-lg border border-border bg-surface-1 px-4 py-2 text-sm text-fg-1">{error}</p>{/if}
  <div
    class="flex items-center gap-2 rounded-2xl border border-border bg-surface-1 px-2 py-1.5 shadow-xl sm:gap-3 sm:rounded-full sm:px-3 {open
      ? 'sheet max-sm:w-full max-sm:flex-col max-sm:items-stretch max-sm:gap-2 max-sm:rounded-b-none max-sm:border-b-0 max-sm:p-3'
      : ''}"
  >
    <button
      type="button"
      class="grid size-8 shrink-0 cursor-grab touch-none place-items-center rounded-full text-fg-2 hover:bg-surface-2 active:cursor-grabbing max-sm:flex max-sm:w-full max-sm:justify-start max-sm:gap-2 max-sm:rounded-lg max-sm:px-1"
      aria-label={open ? 'Collapse agent prompt' : 'Open agent prompt'}
      aria-expanded={open}
      onpointerdown={grab}
      onpointermove={move}
      onpointerup={() => (drag = null)}
      onpointercancel={() => (drag = null)}
      onclick={toggle}
    >
      <Bot class="size-4" />
      <span class="text-xs text-fg-1 sm:hidden">Agent</span>
    </button>
    {#if open}
    <span class="text-xs text-fg-3 max-sm:order-1 max-sm:px-1" role="status">{!activeSession
      ? 'No session'
      : !connected
        ? 'Connecting'
        : feed.status}</span>
    <form
      class="flex w-full min-w-0 items-end gap-1 rounded-2xl border border-input bg-surface-1 p-1 max-sm:order-3 sm:w-80"
      onsubmit={(event) => {
        event.preventDefault();
        void send();
      }}
    >
      <label class="sr-only" for="figcraft-prompt">Agent prompt</label>
      <textarea
        id="figcraft-prompt"
        rows="1"
        class="w-full resize-none bg-transparent px-1.5 py-1 text-[13px] outline-none placeholder:text-fg-4"
        placeholder={activeSession
          ? 'Message the agent. Enter sends, Shift+Enter adds a line.'
          : 'Open this board from a session to prompt it.'}
        bind:value={promptText}
        disabled={!activeSession || sending}
        onkeydown={(event) => {
          if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault();
            void send();
          }
        }}
      ></textarea>
      <button
        type="submit"
        class="icon-btn shrink-0 bg-brand text-on-accent hover:bg-brand-strong"
        aria-label="Send to agent"
        title="Send (Enter)"
        disabled={!activeSession || !promptText.trim() || sending}
      >
        <ArrowUp class="size-4" />
      </button>
    </form>
    {#if activeSession && onbacktofeed}
      <button
        type="button"
        class="icon-btn shrink-0 max-sm:order-4"
        aria-label="Open chat"
        title="Open chat"
        onclick={() => activeSession && onbacktofeed?.(activeSession.id)}
      >
        <MessageSquare class="size-4" />
      </button>
    {/if}
    {/if}
  </div>
</div>

<style>
  /* A phone has no room for the bar: the opened pill becomes a bottom sheet. */
  @media (max-width: 639px) {
    .sheet {
      animation: sheet-up var(--dur-standard) var(--ease-smooth);
    }
  }

  @keyframes sheet-up {
    from {
      transform: translateY(100%);
      opacity: 0.6;
    }
  }
</style>
