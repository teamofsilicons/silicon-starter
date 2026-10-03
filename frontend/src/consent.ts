import {ApiError, validActor, validOrg, type Session} from './api.ts';
export type Consent = {
  request_id: string; context_id: string; completed: boolean; roots?: {endpoint_id?: string; org_id?: string}[];
  authorization: {id: string; app_id: string; actor: Session['actor']; org_id: string; status: string; version: number; expires_at: string; authorization_url?: string; redirect_uri?: string; state?: string};
};
export function consentOf(value: unknown, session: Session, expected?: Consent): Consent {
  const c = value as Consent, a = c?.authorization;
  if (typeof c?.completed !== 'boolean' || !['pending','approved','declined','exchanged','expired'].includes(a?.status) || (c.completed && !['approved','exchanged'].includes(a.status)) || !c?.request_id || c.context_id !== session.context_id || !a?.id || a.app_id !== 'starter' || !validActor(a.actor) || !validOrg(a.org_id) ||
      a.actor?.public_id !== session.actor?.public_id || a.actor?.type !== session.actor?.type || a.org_id !== session.org_id || !Number.isInteger(a.version) || a.version < 1 ||
      !Number.isFinite(Date.parse(a.expires_at)) || (a.redirect_uri && (typeof a.state !== 'string' || a.state.length < 32 || a.state.length > 512)) ||
      (expected && (c.request_id !== expected.request_id || a.id !== expected.authorization.id || a.state !== expected.authorization.state || (expected.completed && !c.completed)))) {
    throw new ApiError('The permission review did not match this account. Start a new review.', 409, 'consent_context_mismatch');
  }
  if (a.authorization_url) {
    const url = new URL(a.authorization_url);
    if (url.protocol !== 'https:' || url.username || url.password) throw new ApiError('IAM returned an invalid review link.', 502, 'invalid_review_url');
  }
  return c;
}
export const requiresFreshReview = (error: unknown) => error instanceof ApiError &&
  (error.status === 412 || (error.status === 403 && error.code === 'reconsent_required'));
export async function completionKey(context: string, request: string, code: string): Promise<string> {
  // Identical retries reconstruct the same key even after a reload; the code itself is never stored.
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(JSON.stringify([context, request, code]))));
  const hex = Array.from(digest.slice(0,16), n => n.toString(16).padStart(2,'0')).join('');
  return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`;
}
