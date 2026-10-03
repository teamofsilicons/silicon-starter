import { ArcButton, ArcInput } from './arc';
import {createSignal, onCleanup, onMount, Show} from 'solid-js';
import {ApiError, type Session} from './api';
import {completionKey, consentOf, requiresFreshReview, type Consent} from './consent';
import {watchPopup} from './popup';

type Request = <T>(path: string, init?: RequestInit) => Promise<T>;
type Receipt = {startKey: string; consent?: Consent; popup?: boolean; returnTo?: string};
export function StoragePermissions(p: {session: Session; api: Request; apiBase: string; login: () => void}) {
  const session = p.session;
  const key = `starter:iam5:briefcase:${JSON.stringify([p.apiBase,session.context_id])}`;
  const [request, setRequest] = createSignal<Consent>();
  const [code, setCode] = createSignal('');
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal('');
  let active = true;
  let operationKey: string | undefined;
  let pendingPopup: ReturnType<typeof watchPopup> | undefined;
  onCleanup(() => { active = false; pendingPopup?.cancel(); setCode(''); });
  const read = (): Receipt | undefined => { try { const r = JSON.parse(localStorage.getItem(key) || 'null'); return typeof r?.startKey === 'string' ? r : undefined; } catch { return undefined; } };
  const save = (receipt: Receipt) => localStorage.setItem(key, JSON.stringify(receipt));
  const accepted = (reply: unknown, receipt: Receipt, completing = false): Consent => {
    const latest = read();
    if (latest?.startKey !== receipt.startKey) throw new ApiError('Another review was started for this account. Reload its status.', 409);
    const current = consentOf(reply, session, latest.consent || receipt.consent);
    if (completing && !current.completed) throw new ApiError('IAM did not confirm completion. Check this request again.', 502);
    save({...receipt, consent:current});
    if (active) setRequest(current);
    return current;
  };
  const failed = (e: unknown) => {
    if (!active) return;
    if (requiresFreshReview(e) && read()?.startKey === operationKey) {
      localStorage.removeItem(key); setRequest(undefined); setCode('');
      setError('This permission needs a fresh review. Your starter and release draft are unchanged.');
    } else setError(e instanceof Error ? e.message : String(e));
  };
  const action = async (run: () => Promise<void>) => {
    if (busy() || !active) return;
    setBusy(true); setError('');
    try { await run(); } catch (e) { failed(e); } finally { if (active) setBusy(false); }
  };
  const openReview = (consent: Consent, popup: Window | null) => {
    const url = new URL(consent.authorization.authorization_url!);
    url.searchParams.set('display', 'popup');
    pendingPopup?.cancel();
    setError('');
    if (!popup) { location.assign(url.href); return; }
    pendingPopup = watchPopup(popup, 'starter:briefcase', async () => {
      const receipt = read();
      if (!receipt?.consent || receipt.consent.request_id !== consent.request_id) throw new Error('This permission review changed. Reload its status.');
      const value = await p.api(`/api/v1/briefcase/authorizations/${encodeURIComponent(consent.request_id)}`);
      accepted(value, receipt, true);
    }, failed);
    pendingPopup.navigate(consent.request_id, url.href);
  };
  const start = () => {
    if (busy() || !active) return;
    const receipt = read() || {startKey:crypto.randomUUID(), popup:true, returnTo:location.pathname + location.search + location.hash};
    const popup = receipt.popup ? window.open('about:blank', '_blank', 'popup,width=600,height=760') : null;
    void action(async () => {
      try {
        save(receipt); operationKey = receipt.startKey;
        const value = await p.api('/api/v1/briefcase/authorization', {method:'POST', headers:{'Idempotency-Key':receipt.startKey}, body:JSON.stringify(receipt.popup ? {popup:true,return_to:receipt.returnTo} : {})});
        const consent = accepted(value, receipt);
        if (receipt.popup && consent.authorization.redirect_uri && consent.authorization.authorization_url && !consent.completed) openReview(consent,popup);
        else popup?.close();
      } catch (error) { popup?.close(); throw error; }
    });
  };
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
    accepted(reply,receipt,true);
    if (active) setCode('');
  });
  const reset = () => { pendingPopup?.cancel(); localStorage.removeItem(key); setRequest(undefined); setCode(''); setError(''); };
  const ended = () => !!request() && (['declined','denied','expired','cancelled'].includes(request()!.authorization.status) || (!request()!.completed && Date.parse(request()!.authorization.expires_at) <= Date.now()));
  onMount(() => {
    const receipt = read();
    if (receipt?.consent) { try { setRequest(consentOf(receipt.consent,session)); void refresh(); } catch { reset(); } }
  });
  return <section class="page-width permissions-page"><span class="eyebrow">ACCOUNT PERMISSIONS</span><h1>Release storage</h1><p>Allow Starter to store release archives in Briefcase. IAM lets you choose the Briefcase account and organization for this feature.</p>
    <Show when={session.authenticated} fallback={<ArcButton class="button primary" onClick={p.login}>Log in to review permissions</ArcButton>}>
      <section class="panel permission-card"><h2>Briefcase</h2><p class="muted">{session.actor?.public_id} · {session.org_id}</p>
        <Show when={request()} fallback={<><p>Storage access is requested separately from signing in. Your starter and release draft stay in place if you decline.</p><ArcButton class="button primary" disabled={busy()} onClick={start}>{busy() ? 'Preparing review…' : read() ? 'Recover permission review' : 'Review storage access'}</ArcButton></>}>
          <Show when={request()?.completed} fallback={<>
            <p role="status">{ended() ? 'This review has ended. You can start again when ready.' : `IAM review: ${request()!.authorization.status}`}</p>
            <Show when={!ended()}><div class="permission-actions"><Show when={request()?.authorization.authorization_url}><Show when={request()?.authorization.redirect_uri} fallback={<a class="button primary" href={request()!.authorization.authorization_url} target="_blank" rel="noopener noreferrer">Open IAM review ↗</a>}><ArcButton class="button primary" onClick={() => openReview(request()!,window.open('about:blank', '_blank', 'popup,width=600,height=760'))}>Open IAM review ↗</ArcButton></Show></Show><ArcButton class="button" disabled={busy()} onClick={refresh}>Check status</ArcButton></div>
              <Show when={!request()?.authorization.redirect_uri}><form onSubmit={event => {event.preventDefault();void complete();}}><label>Code from IAM<ArcInput autocomplete="off" spellcheck={false} value={code()} onInput={event=>setCode(event.currentTarget.value)} /></label><ArcButton class="button" disabled={busy() || !code().trim()}>{busy() ? 'Checking…' : 'Complete authorization'}</ArcButton></form></Show>
            </Show><ArcButton class="button" disabled={busy()} onClick={reset}>Start a new review</ArcButton>
          </>}><p class="notice" role="status">Storage access is ready. Return to your original publish command to continue the release.</p><ArcButton class="button" disabled={busy()} onClick={reset}>Review storage access again</ArcButton></Show>
        </Show>
        <Show when={error()}><p class="notice error" role="alert">{error()}</p></Show>
      </section>
    </Show>
  </section>;
}
