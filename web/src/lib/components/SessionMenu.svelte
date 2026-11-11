<script lang="ts">
  import * as Popover from '$lib/components/ui/popover';
  import Button from '$lib/components/ui/button.svelte';
  import { holdToLift } from '$lib/hold';
  import type { Project, Session } from '$lib/api/types';

  let {
    session,
    project,
    children
  }: {
    session: Session | null;
    project: Project | null;
    children?: import('svelte').Snippet;
  } = $props();

  let open = $state(false);
  let copied = $state('');

  const link = $derived(
    session && project
      ? `${window.location.origin}/p/${project.id}/s/${session.id}`
      : window.location.href
  );
  // A paste-ready block: the title, where the Project lives, and the link the app itself cannot
  // show while installed (it has no URL bar).
  const reference = $derived(
    [
      `Session: ${session?.title || 'Untitled'}`,
      `Project: ${project ? `${project.name} (${project.root})` : 'unknown'}`,
      `URL: ${link}`
    ].join('\n')
  );

  async function copy(text: string, which: string) {
    try {
      await navigator.clipboard.writeText(text);
      copied = which;
    } catch {
      copied = '';
    }
  }

  // Long press (or a right click) on the header opens the menu; a scroll or a drag cancels the hold.
  function press(event: PointerEvent) {
    holdToLift(event, 450, {
      lift: () => (open = true),
      move: () => {},
      drop: () => {}
    });
  }
</script>

<Popover.Root bind:open>
  <Popover.Trigger>
    {#snippet child({ props })}
      <div
        {...props}
        class="min-w-0 flex-1"
        onpointerdown={press}
        oncontextmenu={(event) => {
          event.preventDefault();
          open = true;
        }}
      >
        {@render children?.()}
      </div>
    {/snippet}
  </Popover.Trigger>
  <Popover.Content side="bottom" align="start" class="w-64 gap-2 p-2">
    <Button class="w-full justify-start text-left text-fg-1" onclick={() => void copy(link, 'link')}>
      {copied === 'link' ? 'Copied link' : 'Copy link'}
    </Button>
    <Button class="w-full justify-start text-left text-fg-1" onclick={() => void copy(reference, 'reference')}>
      {copied === 'reference' ? 'Copied reference' : 'Copy session reference'}
    </Button>
  </Popover.Content>
</Popover.Root>
