"use client";

import Link from 'next/link';
import { usePathname, useRouter, useSearchParams } from 'next/navigation';
import { useCallback, useEffect, useEffectEvent, useRef, useState, type FormEvent } from 'react';
import { Button, Dialog, DialogContent, Input, Select } from './silicon-ui';
import { createApi, sessionOf, contextsOf, type Session, type SavedContext } from './api';
import { popupLogin } from './popup';
import { API, api, client, ErrorMessage, Icon, message, type Route } from './shared';
import { RepositoryPage } from './repository-page';
import { BlockPage, PublishBlock, Explore, Publish, Docs } from './catalog-pages';
import { catalogTypes, selectedKind, type CatalogKind } from './catalog';

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

  const routeKind: CatalogKind | '' = route.page === 'starters' ? 'starter' : route.page === 'genes' ? 'gene' : route.page === 'isis' ? 'isi' : route.page === 'functions' ? 'function' : '';
  const isCatalog = ['explore', 'blocks', 'starters', 'genes', 'isis', 'functions'].includes(route.page) && !route.id;
  const activeKind = routeKind || selectedKind(new URLSearchParams(search).get('type'));
  let page;
  if (route.page === 'starters' && route.id) page = <RepositoryPage key={route.id} id={route.id} route={route} session={session} navigate={navigate} login={login} />;
  else if (isCatalog) page = <Explore session={session} search={search} initialKind={routeKind} />;
  else if (route.page === 'docs') page = <Docs />;
  else if (route.page === 'new') page = <Publish session={session} search={search} login={login} navigate={navigate} />;
  else if (route.page === 'blocks' && (route.id === 'new' || route.tab === 'edit')) page = <PublishBlock key={route.id} id={route.id === 'new' ? undefined : route.id} session={session} login={login} navigate={navigate} />;
  else if (route.page === 'blocks' && route.id) page = <BlockPage key={route.id} id={route.id} session={session} />;
  else page = <div className="page-width empty"><h1>Page not found</h1><Link href="/">Explore the registry</Link></div>;

  return <div className="app">
    <a className="skip-link" href="#main-content">Skip to content</a>
    <aside className="app-sidebar">
      <Link className="brand" href="/"><span className="brand-mark" aria-hidden="true">✦</span><span>starter<small>by team of silicons</small></span></Link>
      <div className="sidebar-section-label">YOUR NEXT BEGINNING</div>
      <nav className="sidebar-nav" aria-label="Main navigation"><Link href="/" aria-current={isCatalog && !activeKind ? 'page' : undefined}><Icon name="template" />Everything<span className="nav-shortcut">⌘ K</span></Link>{catalogTypes.map(type => <Link key={type.kind} href={`/?type=${type.kind}`} aria-current={isCatalog && activeKind === type.kind ? 'page' : undefined}><Icon name={type.icon} />{type.plural}<code>{type.prefix}</code></Link>)}</nav>
      <div className="sidebar-divider" /><nav className="sidebar-nav" aria-label="Resources"><Link href="/docs" aria-current={route.page === 'docs' ? 'page' : undefined}><Icon name="book" />Documentation<span aria-hidden="true">↗</span></Link><a href="https://github.com/teamofsilicons/silicon-starter"><Icon name="code" />On GitHub<span aria-hidden="true">↗</span></a></nav>
      <div className="sidebar-bottom"><p>Build on a good idea.<br />Then make it yours.</p><Link className="button primary full-width" href="/new"><Icon name="plus" size={16} />Publish something</Link><a className="silicon-family" href="https://teamofsilicons.com">Part of the Silicon family ↗</a></div>
    </aside>
    <div className="app-main">
    <header className="site-header">
      <Link className="brand mobile-brand" href="/"><span className="brand-mark" aria-hidden="true">✦</span>starter</Link>
      <div className="header-location"><span>Workspace</span><span>/</span><strong>{isCatalog ? 'Explore the registry' : route.page === 'docs' ? 'Documentation' : route.page === 'new' ? 'Publish something' : 'The registry'}</strong></div>
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
        <Link className="button primary" href="/new"><Icon name="plus" size={16} />Publish</Link>
      </>}</div>
    </header>
    <nav className="mobile-nav" aria-label="Registry navigation"><Link href="/" aria-current={isCatalog && !activeKind ? 'page' : undefined}>Everything</Link>{catalogTypes.map(type => <Link key={type.kind} href={`/?type=${type.kind}`} aria-current={isCatalog && activeKind === type.kind ? 'page' : undefined}>{type.plural}</Link>)}<Link href="/docs" aria-current={route.page === 'docs' ? 'page' : undefined}>Docs</Link></nav>
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
    <main id="main-content">{authReady ? page : <div className="page-width loading">{authLoading ? 'Checking session…' : 'Your saved login could not be checked. Retry when the connection is available.'}</div>}</main>
    <footer className="site-footer"><span>Good beginnings, shared.</span><Link href="/docs">Documentation</Link><a href="https://apps.teamofsilicons.com/apps/starter">Silicon Apps ↗</a><a href="https://github.com/teamofsilicons/silicon-starter">GitHub ↗</a></footer>
    </div>
  </div>;
}
