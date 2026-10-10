export type Actor = { uuid: string; kind: 'carbon' | 'silicon'; id: string; display_name?: string; pfp_url?: string };
export type Session = { authenticated: boolean; context_id?: string; actor?: Actor | null; expires_at?: number; access_expires_at?: number };
export type SavedContext = { context_id: string; actor: Actor; selected: boolean; expires_at?: number };
export class ApiError extends Error {
  status: number;
  code: string;
  constructor(message: string, status: number, code = 'request_failed') { super(message); this.status = status; this.code = code; }
}
export function validActor(actor: unknown): actor is Actor {
  if (!actor || typeof actor !== 'object') return false;
  const a = actor as Actor;
  return typeof a.uuid === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(a.uuid) &&
    ((a.kind === 'carbon' && /^c:[a-z0-9_-]+$/.test(a.id)) || (a.kind === 'silicon' && /^si:[a-z0-9_-]+$/.test(a.id)));
}
export function sessionOf(value: unknown): Session {
  const s = value as Session;
  if (s?.authenticated === false) return { authenticated: false };
  if (s?.authenticated !== true || !validActor(s.actor) || typeof s.context_id !== 'string' || !s.context_id) {
    throw new ApiError('This saved login needs to be renewed. Sign in with Silicon Accounts to continue.', 401, 'reauthentication_required');
  }
  return s;
}
export function contextsOf(value: unknown): SavedContext[] {
  const contexts = (value as {contexts?: SavedContext[]})?.contexts;
  if (!Array.isArray(contexts) || contexts.some(c => !c.context_id || !validActor(c.actor))) {
    throw new ApiError('Saved accounts could not be verified.', 502, 'invalid_contexts');
  }
  return contexts;
}

export function createApi(base: string) {
  let context: string | undefined;
  let invalidSession: (() => void) | undefined;
  const changed = () => new ApiError('The selected account changed. Reload this page before continuing.', 409, 'context_changed');
  return {
    context: () => context,
    onSessionInvalid(callback?: () => void) { invalidSession = callback; },
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
        const code = (error as {code?: string})?.code;
        if ((response.status === 401 && code === 'session_expired') || (response.status === 409 && code === 'context_changed')) invalidSession?.();
        throw new ApiError(detail || `Request failed (${response.status})`, response.status, code);
      }
      if (path === '/auth/session' && captured !== undefined) {
        if ((sessionOf(data).context_id || 'anonymous') !== captured) throw changed();
      }
      return data as T;
    },
  };
}
