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
