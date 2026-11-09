import { expect, it, vi } from 'vitest';
import { downloadFigcraftFile, liveInvite } from './figcraft-web';

it('joins the project through the same-origin WebSocket proxy, never the server key', () => {
  const invite = new URL(liveInvite('figcraft://join?host=127.0.0.1:47900&file=design-one&token=edit-secret', 'https://emberloop.example'));
  expect(invite.protocol).toBe('wss:');
  expect(invite.host).toBe('emberloop.example');
  expect(invite.pathname).toBe('/api/figcraft/live');
  expect(invite.searchParams.get('file')).toBe('design-one');
  expect(invite.searchParams.get('token')).toBe('edit-secret');
  expect(invite.searchParams.has('key')).toBe(false);
});

it('rejects missing file credentials instead of opening an unrelated document', () => {
  expect(() => liveInvite('ws://localhost:47900/?key=publish-secret', 'http://localhost:3000')).toThrow();
});

it('keeps upstream file exports working in page-host mode', () => {
  const create = vi.fn(() => 'blob:export');
  Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: create });
  Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: vi.fn() });
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  downloadFigcraftFile(new CustomEvent('figcraft:save', { detail: { name: 'design.svg', bytes: new Uint8Array([60, 62]).buffer } }));
  expect(create).toHaveBeenCalledWith(expect.any(Blob));
  expect(click).toHaveBeenCalledOnce();
  click.mockRestore();
});
