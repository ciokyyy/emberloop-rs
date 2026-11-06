<script lang="ts">
  import { resolveComposerAction, shouldSend } from '$lib/composer';
  import { MAX_IMAGES, pickImages, type ImageAttachment, type RejectReason } from '$lib/images';
  import { collapse, fadeAt, flip, flipAnimation, flipReady, rise, slideDown } from '$lib/motion';
  import { cn } from '$lib/utils';
  import Button from './ui/button.svelte';

  let {
    working = false,
    disabled = false,
    onsubmit,
    oncancel
  }: {
    working?: boolean;
    disabled?: boolean;
    onsubmit: (text: string, files: File[]) => Promise<void> | void;
    oncancel: () => Promise<void> | void;
  } = $props();

  let text = $state('');
  let attachments = $state<ImageAttachment<File>[]>([]);
  let errors = $state<string[]>([]);
  let busy = $state(false);
  let dragging = $state(false);
  let textarea = $state<HTMLTextAreaElement | null>(null);
  let picker = $state<HTMLInputElement | null>(null);

  const action = $derived(resolveComposerAction(working));
  const canSend = $derived(!disabled && !busy && (!!text.trim() || (!working && attachments.length > 0)));

  $effect(() => {
    void text;
    if (!textarea) return;
    textarea.style.height = 'auto';
    textarea.style.height = `${Math.min(textarea.scrollHeight, 200)}px`;
  });

  const reasonText = (reason: RejectReason) =>
    reason === 'size'
      ? 'is over 10 MB and was not attached.'
      : reason === 'type'
        ? 'is not a PNG, JPEG, WebP or GIF image.'
        : `was not attached (limit ${MAX_IMAGES}).`;

  function addFiles(files: Iterable<File>) {
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
      <label class="sr-only" for="composer-text">Message</label>
      <textarea
        id="composer-text"
        bind:this={textarea}
        bind:value={text}
        rows="2"
        class="w-full resize-none bg-transparent px-2 py-1 outline-none"
        placeholder={working
          ? 'Steer the agent. It reaches the model with the next tool result.'
          : 'Message the agent. Enter sends, Shift+Enter adds a line.'}
        {disabled}
        {onkeydown}
        {onpaste}
      ></textarea>

      <div class="flex items-center gap-2 px-1 pt-1">
        {#if !working}
        <Button onclick={() => picker?.click()} aria-label="Attach image">+ Image</Button>
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
        <span class="hide-sm text-[12px] text-fg-3">10 MB each · 5 per prompt</span>
        {/if}
        <span class="flex-1"></span>
        {#if working}
          <span class="inline-flex" transition:fadeAt><Button onclick={() => void oncancel()} disabled={busy}>Stop</Button></span>
        {/if}
        <Button variant={working ? 'default' : 'primary'} onclick={() => void submit()} disabled={!canSend}>
          {action === 'steer' ? 'Steer' : 'Send'}
        </Button>
      </div>
    </div>

    <div class="mt-1.5 px-1 text-[12px] text-fg-3">
      {working
        ? 'Working. A steer is injected at the next request, or sent as a prompt if the turn ends first.'
        : ''}
    </div>
  </div>
</div>
