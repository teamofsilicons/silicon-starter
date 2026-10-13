"use client";

import Link from 'next/link';
import { usePathname, useRouter, useSearchParams } from 'next/navigation';
import { useCallback, useEffect, useEffectEvent, useRef, useState, type FormEvent } from 'react';
import { Button, Dialog, DialogContent, Input, Select } from './silicon-ui';
import { createApi, sessionOf, contextsOf, type Session, type SavedContext } from './api';
import { popupLogin } from './popup';
import { API, api, client, ErrorMessage, Icon, message, type Route } from './shared';
import { RepositoryPage } from './repository-page';
import { Blocks, BlockPage, PublishBlock, Explore, CreateStarter, Docs } from './catalog-pages';

function parseRoute(pathname: string): Route {
  try {
    const parts = pathname.split('/').filter(Boolean).map(decodeURIComponent);
    return { page: parts[0] || 'explore', id: parts[1] || '', tab: parts[2] || 'code', path: parts.slice(3).join('/') };
  } catch { return { page: 'invalid', id: '', tab: '', path: '' }; }
}

export default function StarterApp() {
  const router = useRouter();
  const route = parseRoute(usePathname());
  const search = useSearchParams().toString();
  const navigate = (path: string) => router.push(path);
  const [session, setSession] = useState<Session>({ authenticated: false });
  const [authLoading, setAuthLoading] = useState(true);
  const [authReady, setAuthReady] = useState(false);
  const [authError, setAuthError] = useState('');
  const [contexts, setContexts] = useState<SavedContext[]>([]);
  const [switching, setSwitching] = useState(false);
  const [popupPending, setPopupPending] = useState(false);
  const [loginOpen, setLoginOpen] = useState(false);
  const [siliconLogin, setSiliconLogin] = useState(false);
  const [siliconToken, setSiliconToken] = useState('');
  const [siliconBusy, setSiliconBusy] = useState(false);
  const [siliconError, setSiliconError] = useState('');
  const siliconAttempt = useRef<string | undefined>(undefined);
  const verifyingSession = useRef(false);
  const cancelPopup = useRef<(() => void) | undefined>(undefined);
  const loginFocus = useRef<HTMLElement | null>(null);

  const login = () => {
    if (popupPending) return;
    const trigger = document.activeElement as HTMLElement;
    loginFocus.current = trigger.closest('details')?.querySelector('summary') || trigger;
    setLoginOpen(true);
  };
  const changeLoginOpen = (open: boolean) => {
    if (siliconBusy) return;
    setLoginOpen(open);
    if (!open) {
      setSiliconToken(''); setSiliconError(''); setSiliconLogin(false);
      siliconAttempt.current = undefined;
    }
  };
  const connectSilicon = async (event: FormEvent) => {
    event.preventDefault();
    if (siliconBusy) return;
    setSiliconBusy(true); setSiliconError('');
    siliconAttempt.current ||= crypto.randomUUID();
    try {
      await createApi(API).request('/auth/cli', { method: 'POST', headers: { 'Idempotency-Key': siliconAttempt.current }, body: JSON.stringify({ slt: siliconToken.trim(), browser: true }) });
      const verified = sessionOf(await createApi(API).request('/auth/session'));
      if (!verified.authenticated || verified.actor?.kind !== 'silicon') throw new Error('Silicon Accounts did not verify this Silicon. Request a new token and try again.');
      setSiliconToken(''); location.reload();
    } catch (error) { setSiliconError(message(error)); }
    finally { setSiliconBusy(false); }
  };
  const loginCarbon = () => {
    cancelPopup.current?.();
    changeLoginOpen(false); setAuthError(''); setPopupPending(true);
    cancelPopup.current = popupLogin(API, api, async () => {
      const verified = sessionOf(await createApi(API).request('/auth/session'));
      if (!verified.authenticated || verified.actor?.kind !== 'carbon') throw new Error('Silicon Accounts did not verify the selected account. Please sign in again.');
      location.reload();
    }, (error, retryable) => { if (!retryable) setPopupPending(false); setAuthError(error.message); });
  };
  const recoverLogin = useCallback(async () => {
    if (verifyingSession.current || popupPending || siliconBusy || switching) return;
    verifyingSession.current = true;
    if (!authReady) setAuthLoading(true);
    setAuthError('');
    try {
      const selected = sessionOf(await createApi(API).request<Session>('/auth/session'));
      if (client.context() !== undefined && client.context() !== (selected.context_id || 'anonymous')) {
        location.reload(); return;
      }
      client.establish(selected); setSession(selected); setAuthReady(true);
      if (selected.authenticated) {
        try { setContexts(contextsOf(await api('/auth/contexts'))); }
        catch (error) { setAuthError(message(error)); }
      }
    } catch (error) { setAuthError(`Could not verify your session. ${message(error)}`); }
    finally { verifyingSession.current = false; setAuthLoading(false); }
  }, [authReady, popupPending, siliconBusy, switching]);
  const recoverCurrentSession = useEffectEvent(() => { void recoverLogin(); });
  const resume = useEffectEvent(() => {
    if (document.visibilityState === 'visible' && authReady) void recoverLogin();
  });

  useEffect(() => {
    const expiry = session.expires_at;
    if (!session.authenticated || !expiry) return;
    const timer = window.setTimeout(() => recoverCurrentSession(), Math.min(2_147_483_647, Math.max(1000, expiry * 1000 - Date.now())));
    return () => window.clearTimeout(timer);
  }, [session.authenticated, session.expires_at]);
  useEffect(() => {
    const click = (event: MouseEvent) => {
      const menu = document.querySelector<HTMLDetailsElement>('.account-menu[open]');
      if (menu && !menu.contains(event.target as Node)) menu.open = false;
    };
    const keyboard = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault(); document.querySelector<HTMLInputElement>('input[type="search"]')?.focus();
      }
    };
    const invalid = () => recoverCurrentSession();
    const visible = () => resume();
    client.onSessionInvalid(invalid);
    document.addEventListener('visibilitychange', visible);
    document.addEventListener('click', click);
    window.addEventListener('keydown', keyboard);
    recoverCurrentSession();
    return () => {
      client.onSessionInvalid();
      cancelPopup.current?.();
      document.removeEventListener('visibilitychange', visible);
      document.removeEventListener('click', click);
      window.removeEventListener('keydown', keyboard);
    };
  }, []);

  const selectContext = async (id: string) => {
    if (!id || id === session.context_id || switching || popupPending) return;
    setSwitching(true); setAuthError('');
    try { await api('/auth/context', { method: 'POST', body: JSON.stringify({ context_id: id }) }); location.assign('/'); }
    catch (error) { setAuthError(message(error)); setSwitching(false); }
  };
  const logout = async () => {
    setSwitching(true); setAuthError('');
    try { await api('/auth/logout', { method: 'POST' }); location.assign('/'); }
    catch (error) { setAuthError(message(error)); setSwitching(false); }
  };
  const accountName = session.actor?.display_name || session.actor?.id || (session.authenticated ? 'Signed in' : 'Guest');
  const savedAccounts = contexts.length ? contexts : session.context_id && session.actor ? [{ context_id: session.context_id, actor: session.actor, selected: true }] : [];

  let page;
  if (route.page === 'starters' && route.id) page = <RepositoryPage key={route.id} id={route.id} route={route} session={session} navigate={navigate} login={login} />;
  else if (route.page === 'explore') page = <Explore session={session} search={search} />;
  else if (route.page === 'docs') page = <Docs />;
  else if (route.page === 'new') page = <CreateStarter session={session} login={login} navigate={navigate} />;
  else if (route.page === 'blocks' && (route.id === 'new' || route.tab === 'edit')) page = <PublishBlock key={route.id} id={route.id === 'new' ? undefined : route.id} session={session} login={login} navigate={navigate} />;
  else if (route.page === 'blocks' && route.id) page = <BlockPage key={route.id} id={route.id} session={session} />;
  else if (route.page === 'blocks') page = <Blocks search={search} />;
  else page = <div className="page-width empty"><h1>Page not found</h1><Link href="/">Explore starters</Link></div>;

  return <div className="app">
    <header className="site-header">
      <Link className="brand" href="/"><span className="brand-mark">✦</span>starter</Link>
      <nav aria-label="Main navigation"><Link href="/" aria-current={route.page === 'explore' ? 'page' : undefined}>Explore</Link><Link href="/blocks" aria-current={route.page === 'blocks' ? 'page' : undefined}>Blocks</Link><Link href="/docs" aria-current={route.page === 'docs' ? 'page' : undefined}>Docs</Link></nav>
      <div className="account">{authLoading ? <span className="muted">Checking session…</span> : !authReady ? <Button variant="secondary" onClick={recoverLogin}>Retry session</Button> : <>
        {session.authenticated ? <div className="account-controls">
          <span className="account-name" title={session.actor?.id}>{accountName}</span>
          <Select label="Selected account" value={session.context_id} disabled={switching || popupPending} onValueChange={selectContext} options={savedAccounts.map(context => ({ value: context.context_id, label: context.actor.id }))} />
          <details className="account-menu" onKeyDown={event => { if (event.key === 'Escape') { event.currentTarget.open = false; event.currentTarget.querySelector('summary')?.focus(); } }}>
            <summary className="button" aria-label="Account actions">···</summary>
            <div className="account-menu-body" onClick={event => { const menu = event.currentTarget.closest('details'); if (menu) menu.open = false; }}>
              <Button variant="ghost" disabled={popupPending} onClick={login}>Add account</Button><Button variant="ghost" disabled={switching || popupPending} onClick={logout}>Sign out</Button>
            </div>
          </details>
        </div> : <Button variant="secondary" disabled={popupPending} onClick={login}>Log in</Button>}
        <Link className="button primary" href="/new"><Icon name="plus" size={16} />New starter</Link>
      </>}</div>
    </header>
    <Dialog open={loginOpen} onOpenChange={changeLoginOpen}>
      <DialogContent className="login-dialog" title="Continue with Silicon Accounts" description="Sign in as a Carbon, or connect a Silicon with a short-lived token. You stay signed in until your session expires or you sign out." onCloseAutoFocus={event => { event.preventDefault(); loginFocus.current?.focus(); }}>
        <div className="login-choices"><Button disabled={siliconBusy} onClick={loginCarbon}>Continue as Carbon</Button><Button variant="secondary" disabled={siliconBusy} onClick={() => setSiliconLogin(value => !value)}>Continue as Silicon</Button></div>
        {siliconLogin && <form className="silicon-login" onSubmit={connectSilicon}>
          <p>Request a token in your terminal:</p><pre><code>silicon-accounts login --app starter</code></pre>
          <Input label="Short-lived token" type="password" required autoComplete="off" spellCheck={false} value={siliconToken} onChange={event => { setSiliconToken(event.currentTarget.value); siliconAttempt.current = undefined; }} placeholder="slt_…" />
          {siliconError && <p role="alert" className="notice error">{siliconError}</p>}
          <Button type="submit" loading={siliconBusy}>{siliconBusy ? 'Connecting…' : 'Connect Silicon'}</Button>
        </form>}
        <div className="dialog-actions"><Button variant="secondary" disabled={siliconBusy} onClick={() => changeLoginOpen(false)}>Cancel</Button></div>
      </DialogContent>
    </Dialog>
    {popupPending && <div className="page-width notice" role="status"><p>Complete sign-in in the Silicon Accounts window.</p><Button variant="secondary" onClick={() => cancelPopup.current?.()}>Cancel sign-in</Button></div>}
    {authError && <div className="page-width"><ErrorMessage error={authError} retry={recoverLogin} /></div>}
    <main>{authReady ? page : <div className="page-width loading">{authLoading ? 'Checking session…' : 'Your saved login could not be checked. Retry when the connection is available.'}</div>}</main>
    <footer className="site-footer"><span>✦ Silicon Starter</span><Link href="/docs">Documentation</Link><a href="https://apps.teamofsilicons.com/apps/starter">Silicon Apps ↗</a><a href="https://github.com/teamofsilicons/silicon-starter">GitHub ↗</a></footer>
  </div>;
}
