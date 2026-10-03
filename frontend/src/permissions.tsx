import {createSignal, onCleanup, onMount, Show} from 'solid-js';
import {ApiError, type Session} from './api';
import {completionKey, consentOf, type Consent} from './consent';

type Request = <T>(path: string, init?: RequestInit) => Promise<T>;
type Receipt = {startKey: string; consent?: Consent};
export function StoragePermissions(p: {session: Session; api: Request; apiBase: string; login: () => void}) {
  const session = p.session;
  const key = `starter:iam5:briefcase:${JSON.stringify([p.apiBase,session.context_id])}`;
  const [request, setRequest] = createSignal<Consent>();
  const [code, setCode] = createSignal('');
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal('');
  let active = true;
  let operationKey: string | undefined;
  onCleanup(() => { active = false; setCode(''); });
  const read = (): Receipt | undefined => { try { const r = JSON.parse(localStorage.getItem(key) || 'null'); return typeof r?.startKey === 'string' ? r : undefined; } catch { return undefined; } };
  const save = (receipt: Receipt) => localStorage.setItem(key, JSON.stringify(receipt));
  const accepted = (reply: unknown, receipt: Receipt): Consent => {
    if (read()?.startKey !== receipt.startKey) throw new ApiError('Another review was started for this account. Reload its status.', 409);
    const current = consentOf(reply, session, receipt.consent);
    save({...receipt, consent:current});
    if (active) setRequest(current);
    return current;
  };
  const failed = (e: unknown) => {
    if (!active) return;
    if (e instanceof ApiError && e.status === 412 && read()?.startKey === operationKey) {
      localStorage.removeItem(key); setRequest(undefined); setCode('');
      setError('The permissions changed while you were reviewing them. Start a new review; your starter and release draft are unchanged.');
    } else setError(e instanceof Error ? e.message : String(e));
  };
  const action = async (run: () => Promise<void>) => {
    if (busy() || !active) return;
    setBusy(true); setError('');
    try { await run(); } catch (e) { failed(e); } finally { if (active) setBusy(false); }
  };
  const start = () => action(async () => {
    const receipt = read() || {startKey:crypto.randomUUID()};
    save(receipt); operationKey = receipt.startKey;
    const value = await p.api('/api/v1/briefcase/authorization', {method:'POST', headers:{'Idempotency-Key':receipt.startKey}, body:'{}'});
    accepted(value, receipt);
  });
  const refresh = () => action(async () => {
    const receipt = read();
    if (!receipt?.consent) throw new Error('Start a permission review first.');
    operationKey = receipt.startKey;
    accepted(await p.api(`/api/v1/briefcase/authorizations/${encodeURIComponent(receipt.consent.request_id)}`), receipt);
  });
  const complete = () => action(async () => {
    const receipt = read(), value = code().trim();
    if (!receipt?.consent || !value) throw new Error('Enter the code from this IAM review.');
    operationKey = receipt.startKey;
    const current = receipt.consent;
    const idempotency = await completionKey(session.context_id!, current.request_id, value);
    const reply = await p.api(`/api/v1/briefcase/authorizations/${encodeURIComponent(current.request_id)}/complete`, {method:'POST', headers:{'Idempotency-Key':idempotency}, body:JSON.stringify({code:value})});
    accepted(reply,receipt);
    if (active) setCode('');
  });
  const reset = () => { localStorage.removeItem(key); setRequest(undefined); setCode(''); setError(''); };
  const ended = () => !!request() && (['declined','denied','expired','cancelled'].includes(request()!.authorization.status) || (!request()!.completed && Date.parse(request()!.authorization.expires_at) <= Date.now()));
  onMount(() => {
    const receipt = read();
    if (receipt?.consent) { try { setRequest(consentOf(receipt.consent,session)); void refresh(); } catch { reset(); } }
  });
  return <section class="page-width permissions-page"><span class="eyebrow">ACCOUNT PERMISSIONS</span><h1>Release storage</h1><p>Allow Starter to store release archives in Briefcase. IAM lets you choose the Briefcase account and organization for this feature.</p>
    <Show when={session.authenticated} fallback={<button class="button primary" onClick={p.login}>Log in to review permissions</button>}>
      <section class="panel permission-card"><h2>Briefcase</h2><p class="muted">{session.actor?.public_id} · {session.org_id}</p>
        <Show when={request()} fallback={<><p>Storage access is requested separately from signing in. Your starter and release draft stay in place if you decline.</p><button class="button primary" disabled={busy()} onClick={start}>{busy() ? 'Preparing review…' : read() ? 'Recover permission review' : 'Review storage access'}</button></>}>
          <Show when={request()?.completed} fallback={<>
            <p role="status">{ended() ? 'This review has ended. You can start again when ready.' : `IAM review: ${request()!.authorization.status}`}</p>
            <Show when={!ended()}><div class="permission-actions"><Show when={request()?.authorization.authorization_url}><a class="button primary" href={request()!.authorization.authorization_url} target="_blank" rel="noopener noreferrer">Open IAM review ↗</a></Show><button class="button" disabled={busy()} onClick={refresh}>Check status</button></div>
              <form onSubmit={event => {event.preventDefault();void complete();}}><label>Code from IAM<input autocomplete="off" spellcheck={false} value={code()} onInput={event=>setCode(event.currentTarget.value)} /></label><button class="button" disabled={busy() || !code().trim()}>{busy() ? 'Checking…' : 'Complete authorization'}</button></form>
            </Show><button class="button" disabled={busy()} onClick={reset}>Start a new review</button>
          </>}><p class="notice" role="status">Storage access is ready. Return to your original publish command to continue the release.</p></Show>
        </Show>
        <Show when={error()}><p class="notice error" role="alert">{error()}</p></Show>
      </section>
    </Show>
  </section>;
}
