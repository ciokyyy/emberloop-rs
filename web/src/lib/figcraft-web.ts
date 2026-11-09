import { api } from './api';

declare global {
  interface Window {
    figcraft: { execute(command: string, params: Record<string, unknown>): Promise<unknown> };
  }
}

/** Keep collaboration on Emberloop's origin; never expose the server's publish key. */
export function liveInvite(link: string, origin: string): string {
  const source = new URL(link);
  const file = source.searchParams.get('file');
  const token = source.searchParams.get('token');
  if (!file || !token) throw new Error('FigCraft document has no live edit credentials');
  const target = new URL('/api/figcraft/live', origin);
  target.protocol = target.protocol === 'https:' ? 'wss:' : 'ws:';
  target.searchParams.set('file', file);
  target.searchParams.set('token', token);
  return target.href;
}

/** Start the unmodified upstream WASM editor, not an Emberloop renderer. */
export async function startFigcraft(projectId: string): Promise<void> {
  const design = await api.getProjectDesign(projectId);
  const url = new URL(window.location.href);
  url.searchParams.set('control', '');
  url.hash = '';
  window.history.replaceState(window.history.state, '', url);
  const modulePath = '/figcraft/figcraft-web-bbb0936dcda06852.js';
  const upstream = await import(/* @vite-ignore */ modulePath);
  const canvas = document.getElementById('figcraft_canvas');
  if (!canvas) throw new Error('FigCraft editor canvas is missing');
  await new Promise<void>((resolve, reject) => {
    canvas.addEventListener('figcraft:ready', () => resolve(), { once: true });
    canvas.addEventListener('figcraft:failed', () => reject(new Error('FigCraft editor failed to start')), { once: true });
    upstream.default({ module_or_path: '/figcraft/figcraft-web-bbb0936dcda06852_bg.wasm' }).catch(reject);
  });
  // Opening the project is already consent; URL invites instead open a confirmation dialog.
  await window.figcraft.execute('collab.join', { link: liveInvite(design.joinLink, url.origin) });
}

/** Page-host mode hands native exports back to the host instead of downloading them. */
export function downloadFigcraftFile(event: Event): void {
  const detail = (event as CustomEvent).detail;
  if (!detail || typeof detail.name !== 'string' || !(detail.bytes instanceof ArrayBuffer)) return;
  const url = URL.createObjectURL(new Blob([detail.bytes]));
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = detail.name;
  anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}
