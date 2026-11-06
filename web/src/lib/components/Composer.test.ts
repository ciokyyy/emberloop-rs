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

  it('keeps images and draft on failure', async () => {
    const view = setup({ onsubmit: vi.fn().mockRejectedValue(new Error('Upload failed')) });
    await fireEvent.change(view.container.querySelector('input[type=file]')!, { target: { files: [image()] } });
    await fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    expect(await screen.findByText('Upload failed')).toBeTruthy();
    expect(screen.getByAltText('screen.png')).toBeTruthy();
  });

  it('does not silently discard images when a working session accepts a steer', async () => {
    const onsubmit = vi.fn();
    const view = setup({ onsubmit });
    await fireEvent.change(view.container.querySelector('input[type=file]')!, { target: { files: [image()] } });
    await view.rerender({ working: true });
    await fireEvent.input(screen.getByRole('textbox'), { target: { value: 'look at this' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Steer' }));
    expect(onsubmit).not.toHaveBeenCalled();
    expect(screen.getByAltText('screen.png')).toBeTruthy();
    expect(screen.getByText(/images cannot be sent while/i)).toBeTruthy();
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
