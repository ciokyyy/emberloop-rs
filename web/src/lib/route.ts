// URL grammar: `/p/<project>/s/<session>` for a Session and its `/cache` page, `/p/<project>/<view>`
// for Settings and Providers, and the bare `/<view>` when there is no Project to hang them on yet.

export type View = 'session' | 'new' | 'settings' | 'providers' | 'usage' | 'skills' | 'skill' | 'cache' | 'design';

/** A page hung on a Project, as opposed to a Session, its cache page or the new-Session draft. */
export type Page = Exclude<View, 'session' | 'new' | 'cache' | 'skill'>;

export const sessionPath = (projectId: string, sessionId: string) => `/p/${projectId}/s/${sessionId}`;

/** The prompt-cache page of a Session. */
export const cachePath = (projectId: string, sessionId: string) => `${sessionPath(projectId, sessionId)}/cache`;

/** newPath is the draft: a Session that exists only once the first prompt is sent. */
export const newPath = (projectId: string) => `/p/${projectId}/new`;

/** One Skill's page, hung on the Project whose list it was opened from. */
export const skillPath = (projectId: string, name: string) => `/p/${projectId}/skills/${name}`;

/** Path to the project's FigCraft design system view; the Session it was opened from stays the agent. */
export const designPath = (projectId: string, sessionId?: string | null) =>
  sessionId ? `/p/${projectId}/design?session=${sessionId}` : `/p/${projectId}/design`;

export const viewPath = (projectId: string | null, view: Page) =>
  projectId ? `/p/${projectId}/${view}` : `/${view}`;

/** viewOf names the view the router matched; anything else is a Session. */
export function viewOf(routeId: string | null | undefined): View {
  if (routeId?.endsWith('/new')) return 'new';
  if (routeId?.endsWith('/cache')) return 'cache';
  if (routeId?.endsWith('/settings')) return 'settings';
  if (routeId?.endsWith('/providers')) return 'providers';
  if (routeId?.endsWith('/usage')) return 'usage';
  if (routeId?.endsWith('/skills/[name]')) return 'skill';
  if (routeId?.endsWith('/skills')) return 'skills';
  if (routeId?.endsWith('/design')) return 'design';
  return 'session';
}

/** newSessionProjects is what the sidebar lists: one Project when the URL scopes one, every
 * Project otherwise. */
export function newSessionProjects<T extends { id: string }>(scope: string | null, projects: T[]): T[] {
  return scope ? projects.filter((project) => project.id === scope) : projects;
}
