// Keep one exact callback in memory and the address bar until confirmed or cancelled.
// Never put the IAM credential in localStorage, logs, or rendered error messages.
export const hasLoginCallback = (url: URL) => url.searchParams.has('slt') || url.hash.startsWith('#slts=');

export function cleanCallbackUrl(value: string): string {
  const url = new URL(value);
  url.searchParams.delete('slt');
  url.searchParams.delete('state');
  if (url.hash.startsWith('#slts=')) url.hash = '';
  if (url.pathname === '/auth/callback') url.pathname = '/';
  return url.pathname + url.search + url.hash;
}

function callbackBody(value: string): string {
  const url = new URL(value);
  let slt = url.searchParams.get('slt') || '';
  if (!slt && url.hash.startsWith('#slts=')) {
    let entries: unknown;
    try { entries = JSON.parse(decodeURIComponent(url.hash.slice(6))); }
    catch { throw new Error('The IAM login callback is invalid. Cancel it and sign in again.'); }
    const matches = Array.isArray(entries) ? entries.filter(entry => entry?.app_id === 'starter') : [];
    if (matches.length === 1 && typeof matches[0].slt === 'string') slt = matches[0].slt;
  }
  if (!slt) throw new Error('No login token for starter; log in again with IAM.');
  return JSON.stringify({slt, state:url.searchParams.get('state') || undefined});
}

export function callbackRecovery(value: string, exchange: (body: string) => Promise<unknown>, redact: (url: string) => void) {
  let source: string | undefined = value;
  let body: string | undefined;
  let busy = false;
  const clear = () => {
    if (source === undefined) return;
    const clean = cleanCallbackUrl(source);
    source = undefined; body = undefined;
    redact(clean);
  };
  return {
    async retry() {
      if (busy) throw new Error('This login attempt is already being checked.');
      if (source === undefined) throw new Error('This login attempt is no longer pending.');
      body ??= callbackBody(source);
      busy = true;
      try { await exchange(body); clear(); }
      finally { busy = false; }
    },
    cancel() {
      if (busy) throw new Error('Wait for this login attempt before cancelling.');
      clear();
    },
  };
}
