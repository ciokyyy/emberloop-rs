import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from './index';

afterEach(() => vi.unstubAllGlobals());

describe('api client', () => {
  it('accepts an empty success body from prompt', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 200 })));
    await expect(api.prompt('s1', 'hello', [])).resolves.toBeUndefined();
  });

  it('reports an error without dumping a whole HTML page', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        new Response('<!doctype html><h1>Not found</h1>', { status: 404 })
      )
    );
    await expect(api.listProjects()).rejects.toThrow('404 Not found');
  });
});

describe('usage history', () => {
  it('reads the newest requests of a Session, oldest first', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify([{ seq: 4, at: 1, inputTokens: 10, freshInputTokens: 2, cachedTokens: 8, cacheCreationTokens: 0, outputTokens: 3, hitPercent: 80 }]), {
        status: 200,
        headers: { 'content-type': 'application/json' }
      })
    );
    vi.stubGlobal('fetch', fetchMock);
    const rows = await api.sessionUsage('s1', 50);
    expect(fetchMock.mock.calls[0][0]).toBe('/api/sessions/s1/usage?limit=50');
    expect(rows[0]).toMatchObject({ seq: 4, cachedTokens: 8, hitPercent: 80 });
  });

  it('asks for the server default when no limit is given', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response('[]', { status: 200, headers: { 'content-type': 'application/json' } })
    );
    vi.stubGlobal('fetch', fetchMock);
    await expect(api.sessionUsage('s 1')).resolves.toEqual([]);
    expect(fetchMock.mock.calls[0][0]).toBe('/api/sessions/s%201/usage');
  });
});
