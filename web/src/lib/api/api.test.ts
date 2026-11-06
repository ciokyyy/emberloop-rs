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
