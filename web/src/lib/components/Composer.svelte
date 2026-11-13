<script lang="ts">
  import { onDestroy } from 'svelte';
  import { resolveComposerAction, shouldSend } from '$lib/composer';
  import { MAX_IMAGES, pickImages, type ImageAttachment, type RejectReason } from '$lib/images';
  import { api } from '$lib/api';
  import { insertAt, MAX_AUDIO_BYTES, pickAudioType } from '$lib/voice';
  import { collapse, fadeAt, flip, flipAnimation, flipReady, rise, slideDown } from '$lib/motion';
  import { cn } from '$lib/utils';
  import ContextRing from './ContextRing.svelte';
  import Brand from './Brand.svelte';
  import Icon from './Icon.svelte';

  let {
    sessionId = '',
    working = false,
    disabled = false,
    contextUsed = null,
    contextSize = 0,
    model = '',
    providerIcon = '',
    imageInput = true,
    ondetails,
    onsubmit,
    oncancel
  }: {
    sessionId?: string;
    working?: boolean;
    disabled?: boolean;
    contextUsed?: number | null;
    contextSize?: number;
    model?: string;
    providerIcon?: string;
    imageInput?: boolean;
    ondetails?: (tab: 'model' | 'context') => void;
    onsubmit: (text: string, files: File[]) => Promise<void> | void;
    oncancel: () => Promise<void> | void;
  } = $props();

  let text = $state(readDraft());

  function readDraft() {
    try { return sessionId ? localStorage.getItem(`emberloop-rs:draft:${sessionId}`) ?? '' : ''; }
    catch { return ''; }
  }

  // shortcut: text drafts only, use IndexedDB if attachment persistence is needed.
  $effect(() => {
    if (!sessionId) return;
    try {
      const key = `emberloop-rs:draft:${sessionId}`;
      if (text) localStorage.setItem(key, text);
      else localStorage.removeItem(key);
    } catch { errors = ['Draft could not be saved in this browser.']; }
  });
  let attachments = $state<ImageAttachment<File>[]>([]);
  let errors = $state<string[]>([]);
  let busy = $state(false);
  let dragging = $state(false);
  let textarea = $state<HTMLTextAreaElement | null>(null);
  let picker = $state<HTMLInputElement | null>(null);
  let voice = $state<'idle' | 'recording' | 'transcribing' | 'failed'>('idle');
  let voiceError = $state('');
  let audio = $state<Blob | null>(null);
  let elapsed = $state(0);
  let recorder: MediaRecorder | null = null;
  let stream: MediaStream | null = null;
  let chunks: Blob[] = [];
  const recording = $derived(voice === 'recording');
  const clock = $derived(`${Math.floor(elapsed / 60)}:${String(elapsed % 60).padStart(2, '0')}`);

  const action = $derived(resolveComposerAction(working));
  const canSend = $derived(!disabled && !busy && (!!text.trim() || attachments.length > 0));

  $effect(() => {
    void text;
    if (!textarea) return;
    textarea.style.height = 'auto';
    textarea.style.height = `${Math.min(textarea.scrollHeight, 200)}px`;
  });

  // Keep the screen awake while recording; a sleeping screen ends the capture. The browser drops the
  // lock when the page hides, so ask again when it comes back.
  $effect(() => {
    if (!recording) return;
    let lock: WakeLockSentinel | null = null;
    const request = async () => {
      try { lock = (await navigator.wakeLock?.request('screen')) ?? null; } catch { lock = null; }
    };
    const onVisible = () => { if (document.visibilityState === 'visible' && !lock) void request(); };
    void request();
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      document.removeEventListener('visibilitychange', onVisible);
      void lock?.release().catch(() => {});
    };
  });

  $effect(() => {
    if (voice !== 'recording') return;
    elapsed = 0;
    const timer = setInterval(() => (elapsed += 1), 1000);
    return () => clearInterval(timer);
  });
  const reasonText = (reason: RejectReason) =>
    reason === 'size'
      ? 'is over 10 MB and was not attached.'
      : reason === 'type'
        ? 'is not a PNG, JPEG, WebP or GIF image.'
        : `was not attached (limit ${MAX_IMAGES}).`;

  onDestroy(() => {
    clearAttachments();
    cancelVoice();
  });

  function addFiles(files: Iterable<File>) {
    if (!imageInput) {
      errors = ['This model does not support images. Choose an image-capable model.'];
      return;
    }
    const result = pickImages(attachments, files);
    errors = result.rejected.map(({ name, reason }) => `${name} ${reasonText(reason)}`);
    attachments = [
      ...attachments,
      ...result.accepted.map(({ file }) => ({ file, url: URL.createObjectURL(file) }))
    ];
  }

  function removeAt(index: number) {
    const item = attachments[index];
    if (item?.url) URL.revokeObjectURL(item.url);
    attachments = attachments.filter((_, i) => i !== index);
  }

  function clearAttachments() {
    for (const item of attachments) if (item.url) URL.revokeObjectURL(item.url);
    attachments = [];
  }

  async function submit() {
    if (!canSend) return;
    if (attachments.length && !imageInput) {
      errors = ['This model does not support images. Remove attachments or choose another model.'];
      return;
    }
    busy = true;
    errors = [];
    try {
      await onsubmit(text.trim(), attachments.map((item) => item.file));
      text = '';
      clearAttachments();
    } catch (error) {
      errors = [error instanceof Error ? error.message : String(error)];
    } finally {
      busy = false;
    }
  }

  function onkeydown(event: KeyboardEvent) {
    if (shouldSend(event)) {
      event.preventDefault();
      void submit();
    }
  }

  function onpaste(event: ClipboardEvent) {
    const files = Array.from(event.clipboardData?.files ?? []).filter((file) =>
      file.type.startsWith('image/')
    );
    if (files.length) {
      event.preventDefault();
      addFiles(files);
    }
  }

  function ondrop(event: DragEvent) {
    event.preventDefault();
    dragging = false;
    addFiles(Array.from(event.dataTransfer?.files ?? []));
  }

  // Voice note: record, then Groq transcript goes in the draft for the user to edit and send.
  async function toggleVoice() {
    if (recording) {
      recorder?.stop();
      return;
    }
    if (voice === 'transcribing') return;
    discardVoice();
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const type = pickAudioType((candidate) => MediaRecorder.isTypeSupported(candidate));
      const current = new MediaRecorder(stream, type ? { mimeType: type } : undefined);
      chunks = [];
      current.ondataavailable = (event) => {
        if (event.data.size) chunks.push(event.data);
      };
      current.onstop = () => void finishVoice(current.mimeType);
      current.start();
      recorder = current;
      voice = 'recording';
    } catch (error) {
      releaseMic();
      errors = [error instanceof Error ? error.message : String(error)];
    }
  }

  async function finishVoice(mimeType: string) {
    voice = 'idle';
    releaseMic();
    const blob = new Blob(chunks, { type: mimeType || 'audio/webm' });
    chunks = [];
    if (blob.size > MAX_AUDIO_BYTES) {
      errors = ['Recording is over 25 MB. Record a shorter note.'];
      return;
    }
    await transcribe(blob);
  }

  async function transcribe(blob: Blob) {
    voice = 'transcribing';
    voiceError = '';
    busy = true;
    try {
      const { text: spoken } = await api.transcribe(blob);
      if (!spoken) throw new Error('No speech was recognized.');
      const start = textarea?.selectionStart ?? text.length;
      const end = textarea?.selectionEnd ?? text.length;
      text = insertAt(text, start, end, spoken);
      audio = null;
      voice = 'idle';
    } catch (error) {
      audio = blob;
      voiceError = error instanceof Error ? error.message : String(error);
      voice = 'failed';
    } finally {
      busy = false;
    }
  }

  function discardVoice() {
    audio = null;
    voiceError = '';
    voice = 'idle';
  }

  function releaseMic() {
    stream?.getTracks().forEach((track) => track.stop());
    stream = null;
  }

  function cancelVoice() {
    if (recorder?.state === 'recording') {
      recorder.onstop = null;
      recorder.stop();
    }
    voice = 'idle';
    releaseMic();
  }
</script>

<div class="px-4 pb-4 pt-1">
  <div class="mx-auto max-w-[760px]">
    {#if errors.length}
      <div
        class="mb-2 rounded-xl border border-status-failed px-3 py-1.5 text-[12px]"
        style="background:#2a2222;color:#ffb3ad"
        transition:slideDown
      >
        {#each errors as error, index (index)}<p>{error}</p>{/each}
      </div>
    {/if}

    {#if attachments.length}
      <div class="mb-2 flex flex-wrap gap-2" transition:collapse>
        {#each attachments as item, index (item.url)}
          <div
            class="relative"
            use:flipReady
            animate:flip={flipAnimation()}
            in:rise
            out:fadeAt
          >
            <img
              src={item.url}
              alt={item.file.name}
              class="h-16 w-16 rounded-lg border border-border object-cover"
            />
            <button
              class="absolute -right-2 -top-2 grid h-5 w-5 place-items-center rounded-full border border-border bg-surface-3 text-[12px] leading-none"
              aria-label={`Remove ${item.file.name}`}
              onclick={() => removeAt(index)}
            >×</button>
          </div>
        {/each}
      </div>
    {/if}

    <div
      class={cn('rounded-2xl border border-input bg-surface-1 p-2', dragging && 'border-brand')}
      role="group"
      ondragover={(event) => {
        event.preventDefault();
        dragging = true;
      }}
      ondragleave={() => (dragging = false)}
      {ondrop}
    >
      {#if voice !== 'idle'}
        <div class="flex min-w-0 items-center gap-2 px-2 pb-1 pt-2 text-[12px]" role="status" aria-live="polite">
          <span class={cn('dot animate-pulse', voice === 'transcribing' ? 'bg-status-working' : 'bg-status-failed')}></span>
          <span class="shrink-0 text-fg-2">
            {#if voice === 'recording'}Recording {clock} · tap the mic to stop
            {:else if voice === 'transcribing'}Transcribing…
            {:else}Transcription failed{/if}
          </span>
          {#if voice === 'failed'}
            <span class="min-w-0 flex-1 truncate text-fg-3">{voiceError}</span>
            <button type="button" class="chip shrink-0 hover:bg-surface-3" onclick={() => audio && void transcribe(audio)}>Retry</button>
            <button type="button" class="chip shrink-0 hover:bg-surface-3" onclick={discardVoice}>Discard</button>
          {/if}
        </div>
      {/if}
      <label class="sr-only" for="composer-text">Message</label>
      <textarea
        id="composer-text"
        bind:this={textarea}
        bind:value={text}
        rows="2"
        class="w-full resize-none bg-transparent px-2 py-1 text-base outline-none sm:text-[15px]"
        placeholder={working
          ? 'Steer the agent. It reaches the model with the next tool result.'
          : 'Message the agent. Enter sends, Shift+Enter adds a line.'}
        {disabled}
        {onkeydown}
        {onpaste}
      ></textarea>

      <div class="flex min-w-0 flex-wrap items-center gap-1.5 px-1 pt-1">
        <button type="button" class="inline-flex min-w-0 items-center gap-1.5 rounded-full px-1 py-1 text-[12px] text-fg-3 hover:bg-surface-3" aria-label="Choose model and reasoning effort" onclick={() => ondetails?.('model')}>
          {#if providerIcon}<Brand id={providerIcon} size={16} />{:else}<span class="max-w-[160px] truncate">{model || 'Model'}</span>{/if}
        </button>
        <button type="button" class="inline-flex min-w-0 items-center gap-1 rounded-full hover:bg-surface-3" aria-label="Open context details" onclick={() => ondetails?.('context')}>
          <ContextRing used={contextUsed} size={contextSize} />
        </button>
        <span class="flex-1"></span>
        <input
          bind:this={picker}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          multiple
          hidden
          onchange={(event) => {
            addFiles(Array.from(event.currentTarget.files ?? []));
            event.currentTarget.value = '';
          }}
        />
        <button
          type="button"
          class={cn('icon-btn', recording && 'text-status-failed')}
          aria-label={recording ? 'Stop recording' : 'Record voice note'}
          title={recording ? 'Stop and transcribe' : 'Record voice note'}
          disabled={disabled || busy}
          onclick={() => void toggleVoice()}
        >{#if voice === 'transcribing'}<span class="h-4 w-4 animate-spin rounded-full border-2 border-fg-3 border-t-transparent"></span>{:else}<Icon name="mic" size={16} />{/if}</button>
        <button
          type="button"
          class="icon-btn"
          aria-label="Attach image"
          title={imageInput ? "Attach image · 10 MB each · 5 per prompt" : "This model does not support images"}
          disabled={disabled || !imageInput}
          onclick={() => picker?.click()}
        ><Icon name="paperclip" size={16} /></button>
        {#if working}
          <button
            type="button"
            class="icon-btn text-status-failed"
            aria-label="Stop"
            title="Stop"
            onclick={() => void oncancel()}
            disabled={busy}
            transition:fadeAt
          ><Icon name="square" size={14} /></button>
        {/if}
        <button
          type="button"
          class="icon-btn bg-brand text-on-accent hover:bg-brand-strong disabled:opacity-40"
          aria-label={action === 'steer' ? 'Steer' : 'Send'}
          title={action === 'steer' ? 'Steer (Enter)' : 'Send (Enter)'}
          data-mode={action}
          onclick={() => void submit()}
          disabled={!canSend}
        ><Icon name="arrow-up" size={16} /></button>
      </div>
    </div>
  </div>
</div>
