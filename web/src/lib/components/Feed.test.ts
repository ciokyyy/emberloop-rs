import { render } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { FeedItem, FeedState } from '$lib/feed';
import Feed from './Feed.svelte';

const root = document.documentElement.style;
const state = (items: FeedItem[]) => ({ items, status: 'idle' }) as unknown as FeedState;
const msg = (id: string): FeedItem => ({ id, kind: 'assistant', text: id }) as FeedItem;
let animate: ReturnType<typeof vi.fn>;

beforeEach(() => {
  root.setProperty('--dur-emphasis', '320ms');
  root.setProperty('--dur-standard', '180ms');
  root.setProperty('--ease-smooth', 'cubic-bezier(0.22, 1, 0.36, 1)');
  // jsdom has no Web Animations; Svelte's transitions call element.animate.
  animate = vi.fn(() => ({ cancel() {}, finished: Promise.resolve(), onfinish: null, currentTime: 0 }));
  Element.prototype.animate = animate as never;
});

afterEach(() => {
  for (const name of ['--dur-standard', '--dur-emphasis', '--ease-smooth']) root.removeProperty(name);
});

describe('Feed motion', () => {
  it('animates an item that streams in, not the first render', async () => {
    const view = render(Feed, { feed: state([msg('a'), msg('b'), msg('c'), msg('d')]) });
    expect(animate).not.toHaveBeenCalled();
    await view.rerender({ feed: state([msg('a'), msg('b'), msg('c'), msg('d'), msg('e')]) });
    expect(animate).toHaveBeenCalledTimes(1);
  });

  // Svelte plays a delay animation first, so one transition is one animate call here.
  it('does not animate a bulk load into an empty feed', async () => {
    const view = render(Feed, { feed: state([]) });
    await view.rerender({ feed: state([msg('a'), msg('b'), msg('c'), msg('d')]) });
    expect(animate).not.toHaveBeenCalled();
  });
});

describe('queued steer', () => {
  const at = (view: { container: HTMLElement }, text: string) => view.container.textContent!.indexOf(text);
  const items = (route: 'pending' | 'injected') =>
    [
      { id: 'a', kind: 'assistant', text: 'first' },
      { id: 's', kind: 'steer', text: 'queued words', route },
      { id: 'b', kind: 'assistant', text: 'last' }
    ] as FeedItem[];

  it('waits at the bottom of the feed until it is delivered', async () => {
    const view = render(Feed, { feed: state(items('pending')) });
    expect(at(view, 'queued words')).toBeGreaterThan(at(view, 'last'));
    await view.rerender({ feed: state(items('injected')) });
    expect(at(view, 'queued words')).toBeGreaterThan(at(view, 'first'));
    expect(at(view, 'queued words')).toBeLessThan(at(view, 'last'));
  });
});

describe('paging', () => {
  it('offers an older page only while there is one, and asks for it', async () => {
    const onloadmore = vi.fn(async () => {});
    const view = render(Feed, { feed: state([msg('a')]), hasMore: false, onloadmore });
    expect(view.queryByTestId('load-earlier')).toBeNull();

    await view.rerender({ feed: state([msg('a')]), hasMore: true, onloadmore });
    await view.getByTestId('load-earlier').click();
    expect(onloadmore).toHaveBeenCalledTimes(1);
  });
});
