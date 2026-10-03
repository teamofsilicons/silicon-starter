export type Actor = { type: 'carbon' | 'silicon'; public_id: string; name?: string; display_name?: string };
export type Session = { authenticated: boolean; context_id?: string; org_id?: string; org_ids?: string[]; actor?: Actor | null; world?: string };
export type SavedContext = { context_id: string; org_id: string; actor: Actor; selected: boolean };
export class ApiError extends Error {
  status: number;
  code: string;
  constructor(message: string, status: number, code = 'request_failed') { super(message); this.status = status; this.code = code; }
}
export const validOrg = (org: unknown): org is string => typeof org === 'string' && /^[a-z0-9_-]{3,50}$/.test(org);
export function validActor(actor: unknown): actor is Actor {
  if (!actor || typeof actor !== 'object') return false;
  const a = actor as Actor;
  return (a.type === 'carbon' && /^c:[a-z0-9_-]{3,50}$/.test(a.public_id)) || (a.type === 'silicon' && /^si:[a-z0-9_-]{3,50}$/.test(a.public_id));
}
export function sessionOf(value: unknown): Session {
  const s = value as Session;
  if (s?.authenticated === false) return { authenticated: false };
  if (s?.authenticated !== true || !validOrg(s.org_id) || !validActor(s.actor) || typeof s.context_id !== 'string' || !s.context_id ||
    (s.org_ids !== undefined && (s.org_ids.length !== 1 || s.org_ids[0] !== s.org_id))) {
    throw new ApiError('This saved login needs to be renewed. Sign in with IAM and choose one account and organization.', 401, 'reauthentication_required');
  }
  return s;
}
export function contextsOf(value: unknown): SavedContext[] {
  const contexts = (value as {contexts?: SavedContext[]})?.contexts;
  if (!Array.isArray(contexts) || contexts.some(c => !c.context_id || !validOrg(c.org_id) || !validActor(c.actor))) {
    throw new ApiError('Saved accounts could not be verified.', 502, 'invalid_contexts');
  }
  return contexts;
}

export function createApi(base: string) {
  let context: string | undefined;
  const changed = () => new ApiError('The selected account changed. Reload this page before continuing.', 409, 'context_changed');
  return {
    context: () => context,
    establish(session: Session) {
      const next = sessionOf(session).context_id || 'anonymous';
      // An account selection performs a full navigation so forms, caches and pending work are
      // disposed together. A mounted page never adopts another tab's changed cookie.
      if (context !== undefined && context !== next) throw changed();
      context = next;
    },
    async request<T>(path: string, init?: RequestInit): Promise<T> {
      const captured = context;
      const headers = new Headers(init?.headers);
      if (init?.body !== undefined && !headers.has('content-type')) headers.set('content-type', 'application/json');
      if (captured) headers.set('x-starter-context', captured);
      const response = await fetch(`${base}${path}`, { credentials: 'include', ...init, headers });
      const text = response.status === 204 ? '' : await response.text();
      if (captured !== context) throw changed();
      let data: unknown;
      try { data = text ? JSON.parse(text) : undefined; } catch { data = undefined; }
      if (!response.ok) {
        const error = (data as {error?: unknown})?.error;
        const detail = typeof error === 'string' ? error : (error as {message?: string})?.message;
        throw new ApiError(detail || `Request failed (${response.status})`, response.status, (error as {code?: string})?.code);
      }
      if (path === '/auth/session' && captured !== undefined) {
        if ((sessionOf(data).context_id || 'anonymous') !== captured) throw changed();
      }
      return data as T;
    },
  };
}
