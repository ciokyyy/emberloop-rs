import { describe, expect, it } from 'vitest';
import { sessionPath, viewOf, viewPath } from './route';

describe('route', () => {
  it('builds the paths the Go app uses', () => {
    expect(sessionPath('p1', 's1')).toBe('/p/p1/s/s1');
    expect(viewPath('p1', 'settings')).toBe('/p/p1/settings');
    expect(viewPath(null, 'providers')).toBe('/providers');
  });

  it('names the view from the route the router matched', () => {
    expect(viewOf('/p/[project]/s/[session]')).toBe('session');
    expect(viewOf('/p/[project]/settings')).toBe('settings');
    expect(viewOf('/settings')).toBe('settings');
    expect(viewOf('/providers')).toBe('providers');
    expect(viewOf('/')).toBe('session');
    expect(viewOf(null)).toBe('session');
  });
});
