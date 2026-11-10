<script lang="ts">
  import { onMount } from 'svelte';
  import FigcraftAgentHud from './FigcraftAgentHud.svelte';
  import { downloadFigcraftFile, startFigcraft } from '../figcraft-web';
  import { api } from '../api';
  import type { Project, Session } from '../api/types';

  let { project, sessions, currentSessionId = null, onbacktofeed }: {
    project: Project;
    sessions: Session[];
    currentSessionId?: string | null;
    onbacktofeed?: (sessionId: string) => void;
  } = $props();
  let error = $state('');
  let canvas: HTMLCanvasElement;

  onMount(() => {
    const failed = () => { error = 'FigCraft could not start. Check WebGPU or WebGL2 support.'; };
    canvas.addEventListener('figcraft:save', downloadFigcraftFile);
    canvas.addEventListener('figcraft:failed', failed);
    void startFigcraft(project.id).catch((cause) => { error = String(cause); });
    return () => {
      canvas.removeEventListener('figcraft:save', downloadFigcraftFile);
      canvas.removeEventListener('figcraft:failed', failed);
    };
  });
</script>

<div class="relative min-h-0 w-full flex-1 overflow-hidden bg-surface-0">
  <canvas bind:this={canvas} id="figcraft_canvas" data-host="page" class="absolute inset-0 block h-full w-full touch-none" aria-label="FigCraft design editor"></canvas>
  <div id="figcraft_loading" class="pointer-events-none absolute inset-0 flex items-center justify-center text-fg-3">Loading FigCraft…</div>
  {#if error}
    <div role="alert" class="absolute inset-0 flex items-center justify-center bg-surface-0 p-8 text-fg-1">{error}</div>
  {/if}
  <FigcraftAgentHud
    {project}
    {sessions}
    {currentSessionId}
    onsendprompt={async (sessionId, text) => { await api.prompt(sessionId, text, []); }}
    {onbacktofeed}
  />
</div>
