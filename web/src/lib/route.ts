// URL grammar: `/p/<project>/s/<session>` for a Session, `/p/<project>/<view>` for Settings and
// Providers, and the bare `/<view>` when there is no Project to hang them on yet.

export type View = 'session' | 'settings' | 'providers' | 'usage';

export const sessionPath = (projectId: string, sessionId: string) => `/p/${projectId}/s/${sessionId}`;

export const viewPath = (projectId: string | null, view: Exclude<View, 'session'>) =>
  projectId ? `/p/${projectId}/${view}` : `/${view}`;

/** viewOf names the view the router matched; anything else is a Session. */
export function viewOf(routeId: string | null | undefined): View {
  if (routeId?.endsWith('/settings')) return 'settings';
  if (routeId?.endsWith('/providers')) return 'providers';
  if (routeId?.endsWith('/usage')) return 'usage';
  return 'session';
}

/** newSessionProjects is what the sidebar lists and the new-Session dialog offers: one Project
 * when the URL scopes one, every Project otherwise. The dialog still asks, so the model is picked. */
export function newSessionProjects<T extends { id: string }>(scope: string | null, projects: T[]): T[] {
  return scope ? projects.filter((project) => project.id === scope) : projects;
}
