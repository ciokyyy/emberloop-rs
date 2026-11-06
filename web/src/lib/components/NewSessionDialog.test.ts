import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import NewSessionDialog from './NewSessionDialog.svelte';

const props = { projects: [], models: [], defaultLevel: 'full', oncreate: async () => {} };

describe('NewSessionDialog', () => {
  it('renders its content when open and nothing when closed', async () => {
    const closed = render(NewSessionDialog, { ...props, open: false });
    expect(screen.queryByText('Create')).toBeNull();
    closed.unmount();
    render(NewSessionDialog, { ...props, open: true });
    expect(await screen.findByText('Create')).toBeTruthy();
  });
});
