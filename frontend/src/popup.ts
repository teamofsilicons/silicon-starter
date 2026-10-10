type Attempt = { attempt_id: string; login_url: string; accounts_origin: string };
type Request = <T>(path: string, init?: RequestInit) => Promise<T>;

export function loginDestination(attempt: Attempt): string {
  const url = new URL(attempt.login_url), accounts = new URL(attempt.accounts_origin);
  const local = ['localhost', '127.0.0.1', '[::1]'].includes(accounts.hostname);
  if (typeof attempt.attempt_id !== 'string' || !attempt.attempt_id || accounts.origin !== attempt.accounts_origin || url.origin !== accounts.origin || url.username || url.password ||
    (url.protocol !== 'https:' && !(local && url.protocol === 'http:')) || url.pathname !== '/authorize') {
    throw new Error('Silicon Accounts returned an invalid login destination. Please try again.');
  }
  return url.href;
}

export function popupLogin(apiBase: string, request: Request, complete: () => Promise<void>, failed: (error: Error, retryable?: boolean) => void): () => void {
  // Open before any asynchronous work so the identity button retains its user gesture.
  const popup = window.open('about:blank', '_blank', 'popup,width=520,height=720');
  const returnTo = location.pathname + location.search + location.hash;
  if (!popup) {
    location.assign(`${apiBase}/auth/login?${new URLSearchParams({ identity_kind: 'carbon', return_to: returnTo })}`);
    return () => {};
  }
  const pending = watchPopup(popup, 'starter:login', complete, failed);
  void request<Attempt>('/auth/attempt', { method: 'POST', body: JSON.stringify({ identity_kind: 'carbon', return_to: returnTo, popup: true }) })
    .then(attempt => pending.navigate(attempt.attempt_id, loginDestination(attempt)))
    .catch(error => pending.fail(error instanceof Error ? error : new Error('Could not start Silicon Accounts login.')));
  return pending.cancel;
}

export function watchPopup(popup: Window, type: 'starter:login', complete: () => Promise<void>, failed: (error: Error, retryable?: boolean) => void) {
  let attemptId: string | undefined, finished = false, notifiedError = false;
  const finish = () => {
    if (finished) return false;
    finished = true;
    window.removeEventListener('message', receive);
    window.clearInterval(closed); window.clearTimeout(timeout);
    popup.close();
    return true;
  };
  const fail = (error: Error) => { if (finish()) failed(error); };
  const receive = (event: MessageEvent) => {
    const data = event.data;
    if (event.origin !== location.origin || event.source !== popup || !attemptId || !data || typeof data !== 'object' || Array.isArray(data) ||
      data.type !== type || data.attempt_id !== attemptId || !['complete', 'error'].includes(data.status) ||
      (data.error !== undefined && typeof data.error !== 'string')) return;
    if (data.status === 'error') {
      if (!notifiedError) failed(new Error('Silicon Accounts has not completed this request. Retry in the Silicon Accounts window or cancel. Your work is unchanged.'), true);
      notifiedError = true;
      return;
    }
    if (!finish()) return;
    void complete().catch(error => failed(error instanceof Error ? error : new Error('Could not verify your Silicon Accounts session.')));
  };
  const closed = window.setInterval(() => { if (popup.closed) fail(new Error('Silicon Accounts window closed. Your work is unchanged.')); }, 500);
  const timeout = window.setTimeout(() => fail(new Error('Silicon Accounts request timed out. Your work is unchanged. Please try again.')), 5 * 60 * 1000);
  window.addEventListener('message', receive);
  return {
    navigate(id: string, destination: string) {
      if (finished) return;
      attemptId = id;
      popup.location.replace(destination);
    },
    fail,
    cancel: () => fail(new Error('Silicon Accounts request cancelled. Your work is unchanged.')),
  };
}
