import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import Composer from './Composer.svelte';

function setup(extra = {}) {
  vi.stubGlobal('URL', { createObjectURL: vi.fn(() => 'blob:preview'), revokeObjectURL: vi.fn() });
  return render(Composer, { onsubmit: vi.fn(), oncancel: vi.fn(), ...extra });
}
const image = () => new File(['pixels'], 'screen.png', { type: 'image/png' });

describe('Composer attachments', () => {
  it('sends an image-only prompt and clears its preview after success', async () => {
    const onsubmit = vi.fn();
    const view = setup({ onsubmit });
    await fireEvent.change(view.container.querySelector('input[type=file]')!, { target: { files: [image()] } });
    expect(screen.getByAltText('screen.png')).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(onsubmit).toHaveBeenCalledWith('', [expect.any(File)]));
    await waitFor(() => expect(screen.queryByAltText('screen.png')).toBeNull());
  });

  it('clears text and attachments immediately while send is still in flight', async () => {
    let release!: () => void;
    const onsubmit = vi.fn(() => new Promise<void>((resolve) => { release = resolve; }));
    const view = setup({ onsubmit });
    const box = screen.getByRole('textbox') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'queued text' } });
    await fireEvent.change(view.container.querySelector('input[type=file]')!, { target: { files: [image()] } });
    await fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    expect(box.value).toBe('');
    expect(screen.queryByAltText('screen.png')).toBeNull();
    release();
    await waitFor(() => expect(onsubmit).toHaveBeenCalledWith('queued text', [expect.any(File)]));
  });

  it('keeps images and draft on failure', async () => {
    const view = setup({ onsubmit: vi.fn().mockRejectedValue(new Error('Upload failed')) });
    await fireEvent.change(view.container.querySelector('input[type=file]')!, { target: { files: [image()] } });
    await fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    expect(await screen.findByText('Upload failed')).toBeTruthy();
    expect(screen.getByAltText('screen.png')).toBeTruthy();
  });

  it('sends images with a steer while a session is working', async () => {
    const onsubmit = vi.fn();
    const view = setup({ onsubmit });
    await fireEvent.change(view.container.querySelector('input[type=file]')!, { target: { files: [image()] } });
    await view.rerender({ working: true });
    await fireEvent.input(screen.getByRole('textbox'), { target: { value: 'look at this' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Steer' }));
    await waitFor(() => expect(onsubmit).toHaveBeenCalledWith('look at this', [expect.any(File)]));
    await waitFor(() => expect(screen.queryByAltText('screen.png')).toBeNull());
  });

  it('rejects image attachments for a text-only model', async () => {
    const view = setup({ imageInput: false });
    await fireEvent.paste(screen.getByRole('textbox'), { clipboardData: { files: [image()] } });
    expect(screen.queryByAltText('screen.png')).toBeNull();
    expect(screen.getByText(/does not support images/i)).toBeTruthy();
  });
});

it('restores text drafts per session and clears only after successful send', async () => {
  localStorage.clear();
  let view = setup({ sessionId: 'draft-one' });
  await fireEvent.input(screen.getByRole('textbox'), { target: { value: 'unfinished' } });
  view.unmount();
  view = setup({ sessionId: 'draft-two' });
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('');
  view.unmount();
  view = setup({ sessionId: 'draft-one' });
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('unfinished');
  await fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe(''));
  view.unmount();
  setup({ sessionId: 'draft-one' });
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('');
});

it('holds a screen wake lock while recording a voice note', async () => {
  const release = vi.fn().mockResolvedValue(undefined);
  const request = vi.fn().mockResolvedValue({ release });
  vi.stubGlobal('navigator', {
    mediaDevices: { getUserMedia: vi.fn().mockResolvedValue({ getTracks: () => [] }) },
    wakeLock: { request }
  });
  vi.stubGlobal('MediaRecorder', class {
    static isTypeSupported = () => true;
    mimeType = 'audio/webm';
    state = 'inactive';
    onstop: (() => void) | null = null;
    start() { this.state = 'recording'; }
    stop() { this.state = 'inactive'; this.onstop?.(); }
  });
  const view = setup();
  await fireEvent.click(screen.getByRole('button', { name: 'Record voice note' }));
  await waitFor(() => expect(request).toHaveBeenCalledWith('screen'));
  view.unmount();
  await waitFor(() => expect(release).toHaveBeenCalled());
});

it('retries a voice note whose transcription failed', async () => {
  vi.stubGlobal('navigator', {
    mediaDevices: { getUserMedia: vi.fn().mockResolvedValue({ getTracks: () => [] }) }
  });
  vi.stubGlobal('MediaRecorder', class {
    static isTypeSupported = () => true;
    mimeType = 'audio/webm';
    state = 'inactive';
    onstop: (() => void) | null = null;
    start() { this.state = 'recording'; }
    stop() { this.state = 'inactive'; this.onstop?.(); }
  });
  vi.stubGlobal('fetch', vi.fn()
    .mockRejectedValueOnce(new TypeError('Failed to fetch'))
    .mockResolvedValue({ ok: true, headers: { get: () => 'application/json' }, json: async () => ({ text: 'hello there' }) }));
  setup();
  await fireEvent.click(screen.getByRole('button', { name: 'Record voice note' }));
  await fireEvent.click(await screen.findByRole('button', { name: 'Stop recording' }));
  expect(await screen.findByText('Transcription failed')).toBeTruthy();
  expect(screen.getByText('Failed to fetch')).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await waitFor(() =>
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('hello there')
  );
  expect(screen.queryByText('Transcription failed')).toBeNull();
});

it('starts the recorder so a second mic tap can stop it', async () => {
  vi.stubGlobal('navigator', {
    mediaDevices: { getUserMedia: vi.fn().mockResolvedValue({ getTracks: () => [] }) }
  });
  // Real MediaRecorder throws on stop() before start(), so this stub fails the test when start() is lost.
  vi.stubGlobal('MediaRecorder', class {
    static isTypeSupported = () => true;
    mimeType = 'audio/webm';
    state = 'inactive';
    onstop: (() => void) | null = null;
    start() { this.state = 'recording'; }
    stop() {
      if (this.state !== 'recording') throw new DOMException('Recorder is inactive', 'InvalidStateError');
      this.state = 'inactive';
      this.onstop?.();
    }
  });
  vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Failed to fetch')));
  setup();
  await fireEvent.click(screen.getByRole('button', { name: 'Record voice note' }));
  await fireEvent.click(await screen.findByRole('button', { name: 'Stop recording' }));
  expect(await screen.findByText('Transcription failed')).toBeTruthy();
});

const skills = (names: string[]) => ({
  ok: true,
  headers: { get: () => 'application/json' },
  json: async () => names.map((name) => ({ name, description: `${name} skill`, project: false, origin: 'agent', path: `/skills/${name}` }))
});

describe('Composer slash commands', () => {
  it('opens a skill list on a leading slash and picks one with Enter', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(skills(['ember-flow', 'ember-retro'])));
    setup();
    const box = screen.getByRole('textbox');
    await fireEvent.input(box, { target: { value: '/ember' } });
    expect(await screen.findByRole('option', { name: /ember-flow/ })).toBeTruthy();
    await fireEvent.keyDown(box, { key: 'Enter' });
    expect((box as HTMLTextAreaElement).value).toBe('/ember-flow ');
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
  });

  it('moves the highlight with arrows and picks with Tab', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(skills(['ember-flow', 'ember-retro'])));
    setup();
    const box = screen.getByRole('textbox');
    await fireEvent.input(box, { target: { value: '/ember' } });
    const first = await screen.findByRole('option', { name: /ember-flow/ });
    expect(first.getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(box, { key: 'ArrowDown' });
    const second = screen.getByRole('option', { name: /ember-retro/ });
    expect(second.getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(box, { key: 'ArrowDown' });
    expect(first.getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(second.getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(box, { key: 'Tab' });
    expect((box as HTMLTextAreaElement).value).toBe('/ember-retro ');
  });

  it('sends the expanded prompt after picking a skill', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(skills(['ember-flow'])));
    const onsubmit = vi.fn();
    setup({ onsubmit });
    const box = screen.getByRole('textbox');
    await fireEvent.input(box, { target: { value: '/ember' } });
    await screen.findByRole('option', { name: /ember-flow/ });
    await fireEvent.input(box, { target: { value: '/ember-flow build it' } });
    await fireEvent.keyDown(box, { key: 'Enter' });
    await waitFor(() =>
      expect(onsubmit).toHaveBeenCalledWith('Read `el://skills/ember-flow` and follow it.\n\nbuild it', [])
    );
  });

  it('expands a steer the same way', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(skills(['ember-flow'])));
    const onsubmit = vi.fn();
    setup({ onsubmit, working: true });
    const box = screen.getByRole('textbox');
    await fireEvent.input(box, { target: { value: '/ember' } });
    await screen.findByRole('option', { name: /ember-flow/ });
    await fireEvent.input(box, { target: { value: '/ember-flow keep going' } });
    await fireEvent.keyDown(box, { key: 'Escape' });
    await fireEvent.click(screen.getByRole('button', { name: 'Steer' }));
    await waitFor(() =>
      expect(onsubmit).toHaveBeenCalledWith('Read `el://skills/ember-flow` and follow it.\n\nkeep going', [])
    );
  });

  it('sends the draft as typed and says so when the skill list fails', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Failed to fetch')));
    const onsubmit = vi.fn();
    setup({ onsubmit });
    const box = screen.getByRole('textbox');
    await fireEvent.input(box, { target: { value: '/ember-flow build it' } });
    expect(await screen.findByText('skill list unavailable')).toBeTruthy();
    await fireEvent.keyDown(box, { key: 'Enter' });
    await waitFor(() => expect(onsubmit).toHaveBeenCalledWith('/ember-flow build it', []));
  });
});
