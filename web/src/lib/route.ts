// URL grammar: `/p/<project>/s/<session>` for a Session, `/p/<project>/<view>` for Settings and
// Providers, and the bare `/<view>` when there is no Project to hang them on yet.

export type View = 'session' | 'new' | 'settings' | 'providers' | 'usage';

/** A page hung on a Project, as opposed to a Session or the new-Session draft. */
type Page = Exclude<View, 'session' | 'new'>;

export const sessionPath = (projectId: string, sessionId: string) => `/p/${projectId}/s/${sessionId}`;

/** newPath is the draft: a Session that exists only once the first prompt is sent. */
export const newPath = (projectId: string) => `/p/${projectId}/new`;

export const viewPath = (projectId: string | null, view: Page) =>
  projectId ? `/p/${projectId}/${view}` : `/${view}`;

/** viewOf names the view the router matched; anything else is a Session. */
export function viewOf(routeId: string | null | undefined): View {
  if (routeId?.endsWith('/new')) return 'new';
  if (routeId?.endsWith('/settings')) return 'settings';
  if (routeId?.endsWith('/providers')) return 'providers';
  if (routeId?.endsWith('/usage')) return 'usage';
  return 'session';
}

/** newSessionProjects is what the sidebar lists: one Project when the URL scopes one, every
 * Project otherwise. */
export function newSessionProjects<T extends { id: string }>(scope: string | null, projects: T[]): T[] {
  return scope ? projects.filter((project) => project.id === scope) : projects;
}
