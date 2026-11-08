import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import NewSessionDialog from './NewSessionDialog.svelte';

const props = { projects: [], models: [], oncreate: async () => {} };

describe('NewSessionDialog', () => {
  it('renders its content when open and nothing when closed', async () => {
    const closed = render(NewSessionDialog, { ...props, open: false });
    expect(screen.queryByText('Create')).toBeNull();
    closed.unmount();
    render(NewSessionDialog, { ...props, open: true });
    expect(await screen.findByText('Create')).toBeTruthy();
  });
});


it('includes reasoning effort in the new Session and updates its choices per model', async () => {
  const oncreate = vi.fn();
  render(NewSessionDialog, { ...props, open: true, projects: [{ id: 'p', name: 'P', root: '/p' }], models: [
    { id: 'a', label: 'A', provider: 'pi', kind: 'chat', efforts: ['high'], image_input: true, context_window: 1000, source: 'pi', stale: false, updated_at: 0 },
    { id: 'b', label: 'B', provider: 'pi', kind: 'chat', efforts: ['max'], image_input: false, context_window: 1000, source: 'pi', stale: false, updated_at: 0 }
  ], oncreate });
  await fireEvent.click(screen.getByRole('button', { name: 'Show model options' }));
  await fireEvent.pointerUp(screen.getByRole('option', { name: /B ·/ }));
  await fireEvent.click(screen.getByRole('combobox', { name: 'Reasoning effort' }));
  await fireEvent.pointerUp(screen.getByRole('option', { name: 'max' }));
  await fireEvent.click(screen.getByText('Create'));
  expect(oncreate).toHaveBeenCalledWith(expect.objectContaining({ model: 'b', thoughtLevel: 'max' }));
});

it('uses the configured default instead of the first model', async () => {
  const oncreate = vi.fn();
  render(NewSessionDialog, { ...props, open: true, projects: [{ id: 'p', name: 'P', root: '/p' }], defaultModel: 'b', models: [
    { id: 'a', label: 'A', provider: 'pi', source: 'pi', kind: 'chat', efforts: [], image_input: false, context_window: 1000, stale: false, updated_at: 0 },
    { id: 'b', label: 'B', provider: 'pi', source: 'pi', kind: 'chat', efforts: ['max'], image_input: false, context_window: 1000, stale: false, updated_at: 0 }
  ], oncreate });
  await fireEvent.click(screen.getByText('Create'));
  expect(oncreate).toHaveBeenCalledWith(expect.objectContaining({ model: 'b', thoughtLevel: 'max' }));
});
