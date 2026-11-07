import { describe, expect, it } from 'vitest';
import { cachePath, newPath, newSessionProjects, sessionPath, viewOf, viewPath } from './route';

describe('route', () => {
  it('builds the paths the Go app uses', () => {
    expect(sessionPath('p1', 's1')).toBe('/p/p1/s/s1');
    expect(viewPath('p1', 'settings')).toBe('/p/p1/settings');
    expect(viewPath(null, 'providers')).toBe('/providers');
    expect(newPath('p1')).toBe('/p/p1/new');
    expect(cachePath('p1', 's1')).toBe('/p/p1/s/s1/cache');
  });

  it('names the view from the route the router matched', () => {
    expect(viewOf('/p/[project]/s/[session]')).toBe('session');
    expect(viewOf('/p/[project]/new')).toBe('new');
    expect(viewOf('/p/[project]/s/[session]/cache')).toBe('cache');
    expect(viewOf('/p/[project]/settings')).toBe('settings');
    expect(viewOf('/settings')).toBe('settings');
    expect(viewOf('/providers')).toBe('providers');
    expect(viewOf('/')).toBe('session');
    expect(viewOf(null)).toBe('session');
  });
});

describe('newSessionProjects', () => {
  const projects = [
    { id: 'p1', name: 'One', root: '/tmp/one' },
    { id: 'p2', name: 'Two', root: '/tmp/two' }
  ];

  it('offers only the Project the URL scopes, and every Project without one', () => {
    expect(newSessionProjects('p2', projects).map((project) => project.id)).toEqual(['p2']);
    expect(newSessionProjects(null, projects)).toBe(projects);
  });
});
