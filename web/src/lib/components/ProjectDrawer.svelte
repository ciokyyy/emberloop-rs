<script lang="ts">
  import { PenTool, ArrowUpRight, X } from '@lucide/svelte';
  import type { Project } from '../api/types';
  import * as Drawer from './ui/drawer';
  import Button from './ui/button.svelte';

  let { project, open, onclose, onopendesign }: {
    project: Project;
    open: boolean;
    onclose: () => void;
    onopendesign: () => void;
  } = $props();
</script>

<Drawer.Root {open} direction="right" onOpenChange={(next) => { if (!next) onclose(); }}>
  <Drawer.Content class="bg-bg-1">
    <Drawer.Header class="flex flex-row items-center justify-between border-b border-border">
      <div>
        <Drawer.Title>{project.name}</Drawer.Title>
        <Drawer.Description>Project tools</Drawer.Description>
      </div>
      <Button aria-label="Close project tools" onclick={onclose}><X class="size-4" /></Button>
    </Drawer.Header>
    <div class="p-4">
      <Button class="flex w-full items-center gap-3 rounded-lg p-3 text-left" onclick={() => { onclose(); onopendesign(); }}>
        <PenTool class="size-5 text-fg-2" />
        <span class="flex-1">FigCraft</span>
        <ArrowUpRight class="size-4 text-fg-3" />
      </Button>
    </div>
  </Drawer.Content>
</Drawer.Root>
