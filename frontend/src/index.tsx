import { render } from 'solid-js/web';
import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from 'solid-js';
import type { JSX } from 'solid-js';
import './styles.css';

type Starter = { id: string; name: string; description: string; owner: string; visibility: 'public' | 'private'; version: string; downloads: number; stars: number; updated_at: string; tags: string[]; yaml: string };
type Version = { version: string; commit: string; notes: string; published_at: string };
type Discussion = { id: string; parent_id?: string; author: string; body: string; created_at: string };
type Session = { authenticated: boolean; org_id?: string | null; org_ids?: string[]; actor?: { name?: string; display_name?: string; id?: string } | string | null };
type RepoFile = { path: string; content: string | null; size?: number; reason?: string };
type Repository = { commit: string | null; files: RepoFile[]; draft?: boolean; truncated?: boolean };
type Route = { page: string; id: string; tab: string; path: string };
const API = (import.meta as ImportMeta & { env: { VITE_API_URL?: string } }).env.VITE_API_URL || '';
const message = (error: unknown) => error instanceof Error ? error.message : String(error);
const api = async <T,>(path: string, init?: RequestInit): Promise<T> => {
  const response = await fetch(`${API}${path}`, { credentials: 'include', ...init, headers: { 'Content-Type': 'application/json', ...init?.headers } });
  if (!response.ok) {
    const body = await response.text();
    let detail = body;
    try { detail = JSON.parse(body).error || body; } catch { /* A proxy may return plain text. */ }
    throw new Error(detail || `Request failed (${response.status})`);
  }
  return response.json();
};
const repoUrl = (id: string, tab = 'code', path = '') => `/starters/${encodeURIComponent(id)}${tab === 'code' ? '' : `/${tab}`}${path ? `/${path.split('/').map(encodeURIComponent).join('/')}` : ''}`;
const parseRoute = (): Route => {
  try {
    const parts = location.pathname.split('/').filter(Boolean).map(decodeURIComponent);
    return { page: parts[0] || 'explore', id: parts[1] || '', tab: parts[2] || 'code', path: parts.slice(3).join('/') };
  } catch { return { page: 'invalid', id: '', tab: '', path: '' }; }
};
const timeAgo = (value: string) => {
  const days = Math.max(0, Math.floor((Date.now() - new Date(value).getTime()) / 86400000));
  return days === 0 ? 'today' : days === 1 ? 'yesterday' : `${days} days ago`;
};
const sizeLabel = (file: RepoFile) => {
  const bytes = file.size ?? (file.content === null ? null : new TextEncoder().encode(file.content).length);
  return bytes === null ? 'Size unavailable' : bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KB`;
};
const Icon = (props: { name: string; size?: number }) => {
  const paths: Record<string, string> = {
    code: 'm8 7-5 5 5 5m8-10 5 5-5 5m-3-14-2 18', folder: 'M3 7V5h7l2 2h9v13H3V7Z', file: 'M6 3h8l4 4v14H6V3Zm8 0v5h4',
    star: 'm12 3 2.8 5.67 6.2.9-4.5 4.4 1.06 6.2L12 18.25 6.44 21.17 7.5 14.97 3 10.57l6.2-.9L12 3Z',
    architecture: 'M9 3h6v6H9V3ZM3 15h6v6H3v-6Zm12 0h6v6h-6v-6ZM12 9v3m-6 3v-3h12v3',
    releases: 'M3 3h8l10 10-8 8L3 11V3Zm4 4h.01', discussions: 'M4 4h16v12H9l-5 4V4Z',
    download: 'M12 3v12m0 0 5-5m-5 5-5-5M4 18v3h16v-3', search: 'M11 19a8 8 0 1 1 0-16 8 8 0 0 1 0 16Zm6-2 4 4',
    book: 'M4 5a2 2 0 0 1 2-2h14v18H6a2 2 0 0 1-2-2V5Zm0 12h16', plus: 'M12 5v14M5 12h14', copy: 'M9 9h12v12H9V9ZM3 15V3h12',
  };
  return <svg width={props.size || 18} height={props.size || 18} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[props.name] || paths.file} /></svg>;
};
function ErrorMessage(props: { error: unknown; retry?: () => void }) {
  return <div class="notice error" role="alert">{message(props.error)}<Show when={props.retry}><button onClick={() => props.retry?.()}>Try again</button></Show></div>;
}
function CommentThread(props: { item: Discussion; all: Discussion[]; ancestors?: string[] }) {
  const ancestors = () => [...(props.ancestors || []), props.item.id];
  return <article class="comment"><div class="comment-heading"><span class="avatar">{props.item.author.slice(0, 1).toUpperCase()}</span><strong>{props.item.author}</strong><time title={props.item.created_at}>{timeAgo(props.item.created_at)}</time></div><p>{props.item.body}</p><div class="replies"><For each={props.all.filter(child => child.parent_id === props.item.id && !ancestors().includes(child.id))}>{child => <CommentThread item={child} all={props.all} ancestors={ancestors()} />}</For></div></article>;
}

// ponytail: render common README blocks as safe JSX; use a Markdown parser when full CommonMark is needed.
function Readme(props: { content: string; link: (path: string) => string }) {
  const inline = (text: string): JSX.Element => text.split(/(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\([^\s)]+\))/g).map(part => {
    if (part.startsWith('`')) return <code>{part.slice(1, -1)}</code>;
    if (part.startsWith('**')) return <strong>{part.slice(2, -2)}</strong>;
    const link = /^\[([^\]]+)\]\(([^\s)]+)\)$/.exec(part);
    if (link) {
      const href = /^(https?:|mailto:)/i.test(link[2]) ? link[2] : /^[a-z][a-z\d+.-]*:|^\/\//i.test(link[2]) ? '' : props.link(link[2]);
      return href ? <a href={href}>{link[1]}</a> : link[1];
    }
    return part;
  });
  const blocks = createMemo(() => {
    const lines = props.content.split('\n');
    const nodes: JSX.Element[] = [];
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      if (/^\s*```/.test(line)) {
        const code: string[] = [];
        while (++i < lines.length && !/^\s*```/.test(lines[i])) code.push(lines[i]);
        nodes.push(<pre><code>{code.join('\n')}</code></pre>);
      } else if (/^###\s/.test(line)) nodes.push(<h3>{inline(line.slice(4))}</h3>);
      else if (/^##\s/.test(line)) nodes.push(<h2>{inline(line.slice(3))}</h2>);
      else if (/^#\s/.test(line)) nodes.push(<h1>{inline(line.slice(2))}</h1>);
      else if (/^[-*]\s/.test(line)) {
        const items = [line.slice(2)];
        while (i + 1 < lines.length && /^[-*]\s/.test(lines[i + 1])) items.push(lines[++i].slice(2));
        nodes.push(<ul>{items.map(item => <li>{inline(item)}</li>)}</ul>);
      } else if (line.trim()) nodes.push(<p>{inline(line)}</p>);
    }
    return nodes;
  });
  return <div class="markdown">{blocks()}</div>;
}

function Architecture(props: { yaml: string }) {
  const blocks = createMemo(() => {
    const blocks: { name: string; body: string }[] = [];
    for (const line of props.yaml.split('\n')) {
      const section = /^(silicon|isi|access|flow):/.exec(line);
      if (section || !blocks.length) blocks.push({ name: section?.[1] || 'configuration', body: line });
      else blocks[blocks.length - 1].body += `\n${line}`;
    }
    return blocks.filter(block => block.body.trim());
  });
  return <div class="architecture-blocks"><For each={blocks()}>{block => <section class={`architecture-block panel ${block.name}`}><header><span></span>{block.name}</header><pre class="architecture-source"><code>{block.body}</code></pre></section>}</For></div>;
}

function RepositoryPage(props: { id: string; route: Route; session: Session; navigate: (path: string) => void; login: () => void }) {
  const [starter, { refetch: reloadStarter, mutate }] = createResource(() => props.id, id => api<Starter>(`/api/v1/starters/${encodeURIComponent(id)}`));
  const [repository, { refetch: reloadFiles }] = createResource(() => props.id, id => api<Repository>(`/api/v1/starters/${encodeURIComponent(id)}/files`));
  const [versions, { refetch: reloadVersions }] = createResource(() => props.id, id => api<Version[]>(`/api/v1/starters/${encodeURIComponent(id)}/versions`).then(items => items.sort((a, b) => +new Date(b.published_at) - +new Date(a.published_at))));
  const [discussions, { refetch: reloadDiscussions }] = createResource(() => props.route.tab === 'discussions' ? props.id : false, id => api<Discussion[]>(`/api/v1/starters/${encodeURIComponent(id)}/discussions`));
  const [notice, setNotice] = createSignal('');
  const [actionError, setActionError] = createSignal('');
  const [comment, setComment] = createSignal('');
  const [posting, setPosting] = createSignal(false);
  const [filter, setFilter] = createSignal('');
  const files = () => repository.error ? [] : repository()?.files || [];
  const tab = () => ['code', 'tree', 'blob'].includes(props.route.tab) ? 'code' : props.route.tab;
  const folder = () => props.route.tab === 'tree' ? props.route.path : '';
  const selected = () => props.route.tab === 'blob' ? files().find(file => file.path === props.route.path) : undefined;
  const entries = createMemo(() => {
    const prefix = folder() ? `${folder()}/` : '';
    const entries = new Map<string, { name: string; path: string; directory: boolean; file?: RepoFile }>();
    for (const file of files()) {
      if (!file.path.startsWith(prefix)) continue;
      const remainder = file.path.slice(prefix.length);
      const name = remainder.split('/')[0];
      if (!name) continue;
      entries.set(name, { name, path: prefix + name, directory: remainder.includes('/'), file: remainder.includes('/') ? undefined : file });
    }
    return [...entries.values()].filter(entry => entry.name.toLowerCase().includes(filter().toLowerCase())).sort((a, b) => Number(b.directory) - Number(a.directory) || a.name.localeCompare(b.name));
  });
  const breadcrumbs = () => props.route.path.split('/').filter(Boolean);
  const readme = () => files().find(file => file.path.slice(0, file.path.lastIndexOf('/') + 1) === (folder() ? `${folder()}/` : '') && /^readme(?:\.md|\.markdown|\.txt)?$/i.test(file.path.split('/').pop() || ''));
  createEffect(() => { props.route.path; props.route.tab; setFilter(''); setNotice(''); setActionError(''); });
  const copy = async (text: string, label: string) => {
    try { await navigator.clipboard.writeText(text); setNotice(`${label} copied.`); } catch { setActionError('Clipboard access was denied. Select and copy the text directly.'); }
  };
  const archive = async (ref?: string) => {
    try {
      setActionError('');
      const payload = await api<{ bundle_base64: string }>(`/api/v1/starters/${encodeURIComponent(props.id)}/archive${ref ? `?ref=${encodeURIComponent(ref)}` : ''}`);
      const bytes = Uint8Array.from(atob(payload.bundle_base64), character => character.charCodeAt(0));
      const url = URL.createObjectURL(new Blob([bytes], { type: 'application/octet-stream' }));
      const anchor = document.createElement('a');
      anchor.href = url; anchor.download = `${props.id}.bundle`; anchor.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (error) { setActionError(message(error)); }
  };
  const star = async () => {
    if (!props.session.authenticated) { props.login(); return; }
    try { mutate(await api<Starter>(`/api/v1/starters/${encodeURIComponent(props.id)}/star`, { method: 'POST' })); setNotice('Starter starred.'); }
    catch (error) { setActionError(message(error)); }
  };
  const post = async (event: SubmitEvent) => {
    event.preventDefault();
    if (!comment().trim() || posting()) return;
    setPosting(true); setActionError('');
    try { await api(`/api/v1/starters/${encodeURIComponent(props.id)}/discussions`, { method: 'POST', body: JSON.stringify({ body: comment().trim() }) }); setComment(''); await reloadDiscussions(); }
    catch (error) { setActionError(message(error)); }
    finally { setPosting(false); }
  };
  const readmeLink = (path: string) => {
    if (path.startsWith('#')) return path;
    const normalized = new URL(path, `https://repository.invalid/${folder() ? `${folder()}/` : ''}`).pathname.slice(1);
    return repoUrl(props.id, files().some(file => file.path === normalized) ? 'blob' : 'tree', normalized);
  };
  return <Show when={!starter.error} fallback={<section class="page-width"><a href="/">← Explore starters</a><ErrorMessage error={starter.error} retry={reloadStarter} /></section>}>
    <Show when={starter()} fallback={<div class="page-width loading">Loading starter…</div>}>{item => <>
      <section class="repository-heading"><div class="page-width repository-title"><div class="repository-name"><Icon name="book" size={23} /><a href={`/?org=${encodeURIComponent(item().owner)}`}>{item().owner}</a><span>/</span><h1><a href={repoUrl(item().id)}>{item().name}</a></h1><span class="badge">{item().visibility}</span></div><button class="button" onClick={star}><Icon name="star" size={16} /> Star <span class="count">{item().stars}</span></button></div>
        <nav class="page-width repository-tabs" aria-label="Starter sections"><For each={[['code', 'Code'], ['architecture', 'Architecture'], ['releases', 'Releases'], ['discussions', 'Discussions']]}>{([name, label]) => <a href={repoUrl(props.id, name)} class={tab() === name ? 'active' : ''} aria-current={tab() === name ? 'page' : undefined}><Icon name={name} />{label}<Show when={name === 'releases' && !versions.error && versions()?.length}><span class="count">{versions()?.length}</span></Show></a>}</For></nav>
      </section>
      <div class="page-width repository-body"><Show when={notice()}><div class="notice" role="status">{notice()}</div></Show><Show when={actionError()}><ErrorMessage error={actionError()} /></Show>
        <Show when={tab() === 'code'}><div class="repository-layout"><div class="repository-main">
          <div class="code-toolbar"><span class="revision"><Icon name="code" size={16} />{repository.error ? 'Source' : repository()?.draft ? 'Draft' : 'Latest source'}</span><Show when={!versions.error}><a href={repoUrl(props.id, 'releases')} class="release-link"><Icon name="releases" size={16} />{versions()?.length || 0} releases</a></Show><details class="code-menu"><summary class="button primary"><Icon name="code" size={16} /> Code <span>⌄</span></summary><div class="code-menu-content"><strong>Use this starter</strong><label for="pull-command">Pull with the CLI</label><div class="copy-command"><input id="pull-command" readonly value={`starter pull ${props.id}`} /><button class="button" aria-label="Copy pull command" onClick={() => copy(`starter pull ${props.id}`, 'CLI command')}><Icon name="copy" size={15} /></button></div><button class="button full-width" onClick={() => archive()}><Icon name="download" size={16} />Download Git bundle</button></div></details></div>
          <Show when={props.route.path}><nav class="file-breadcrumbs" aria-label="File path"><a href={repoUrl(props.id)}>{props.id}</a><For each={breadcrumbs()}>{(part, index) => <><span>/</span><Show when={index() < breadcrumbs().length - 1} fallback={<strong>{part}</strong>}><a href={repoUrl(props.id, 'tree', breadcrumbs().slice(0, index() + 1).join('/'))}>{part}</a></Show></>}</For></nav></Show>
          <Show when={!repository.error} fallback={<ErrorMessage error={repository.error} retry={reloadFiles} />}><Show when={!repository.loading} fallback={<div class="loading panel">Reading repository files…</div>}>
            <Show when={repository()?.truncated}><div class="notice">This repository is too large to preview completely. Download the Git bundle for all files.</div></Show>
            <Show when={props.route.tab === 'blob'} fallback={<>
              <Show when={!folder() || files().some(file => file.path.startsWith(`${folder()}/`))} fallback={<div class="empty panel"><h2>Directory not found</h2><a href={repoUrl(props.id)}>Return to repository root</a></div>}>
                <div class="file-table panel"><div class="source-heading"><strong>{repository()?.draft ? 'Organization draft' : 'Repository files'}</strong><Show when={repository()?.commit}><code title={repository()?.commit || undefined}>{repository()?.commit?.slice(0, 10)}</code></Show><span>{files().length} files</span></div><Show when={files().length > 12}><div class="file-filter"><Icon name="search" size={16} /><input aria-label="Filter this directory" placeholder="Filter this directory…" value={filter()} onInput={event => setFilter(event.currentTarget.value)} /></div></Show><Show when={folder()}><a class="file-row parent-folder" href={repoUrl(props.id, folder().includes('/') ? 'tree' : 'code', folder().split('/').slice(0, -1).join('/'))}><Icon name="folder" />..</a></Show><For each={entries()} fallback={<div class="empty compact">{filter() ? 'No files match this filter.' : 'No source files are available.'}</div>}>{entry => <a class="file-row" href={repoUrl(props.id, entry.directory ? 'tree' : 'blob', entry.path)}><Icon name={entry.directory ? 'folder' : 'file'} /><span>{entry.name}</span><small>{entry.directory ? 'Directory' : entry.file && sizeLabel(entry.file)}</small></a>}</For></div>
                <Show when={readme()?.content != null}><section class="readme panel"><header><Icon name="book" size={16} /><a href={repoUrl(props.id, 'blob', readme()!.path)}>{readme()!.path.split('/').pop()}</a></header><Readme content={readme()!.content!} link={readmeLink} /></section></Show>
              </Show>
            </>}>
              <Show when={selected()} fallback={<div class="empty panel"><h2>File not found</h2><a href={repoUrl(props.id)}>Return to repository root</a></div>}>{file => <section class="file-view panel"><header><div><strong>{file().path.split('/').pop()}</strong><span>{sizeLabel(file())}</span><Show when={file().content !== null}><span>{file().content!.split('\n').length} lines</span></Show></div><Show when={file().content !== null}><button class="button" onClick={() => copy(file().content!, 'File')}>Copy</button></Show></header><Show when={file().content !== null} fallback={<div class="empty"><h2>Preview unavailable</h2><p>{({ binary: 'Binary file. Download the Git bundle to open it.', large: 'This file is too large to preview. Download the Git bundle to open it.', response_limit: 'The repository preview size limit was reached. Download the Git bundle to open this file.', symlink: 'Symbolic link. Download the Git bundle to inspect its target.', submodule: 'Git submodule. Download the Git bundle to inspect its reference.' } as Record<string, string>)[file().reason || ''] || 'This file cannot be displayed as text. Download the Git bundle to open it.'}</p></div>}><div class="code-scroll"><pre class="source-code"><For each={file().content!.split('\n')}>{(line, index) => <div class="source-line" id={`L${index() + 1}`}><a class="line-number" href={`#L${index() + 1}`} aria-label={`Line ${index() + 1}`}>{index() + 1}</a><code>{line || '\n'}</code></div>}</For></pre></div></Show></section>}</Show>
            </Show>
          </Show></Show>
        </div><aside class="repository-about"><h2>About</h2><p>{item().description || 'No description provided.'}</p><div class="tags"><For each={item().tags}>{tag => <a href={`/?q=${encodeURIComponent(tag)}`}>{tag}</a>}</For></div><dl><div><dt>Organization</dt><dd><a href={`/?org=${encodeURIComponent(item().owner)}`}>{item().owner}</a></dd></div><div><dt>Starter ID</dt><dd><code>{item().id}</code></dd></div></dl><p class="about-stat"><Icon name="star" size={16} />{item().stars} stars</p><p class="about-stat"><Icon name="download" size={16} />{item().downloads.toLocaleString()} downloads</p><p class="muted">Updated {timeAgo(item().updated_at)}</p><div class="about-releases"><h2><a href={repoUrl(props.id, 'releases')}>Releases</a><Show when={!versions.error}><span class="count">{versions()?.length || 0}</span></Show></h2><Show when={!versions.error} fallback={<p class="muted">Release history unavailable.</p>}><Show when={versions()?.[0]} fallback={<p class="muted">No releases published.</p>}>{version => <><a class="latest-release" href={repoUrl(props.id, 'releases')}><Icon name="releases" size={17} />v{version().version}<span class="latest-badge">Latest</span></a><p class="muted">{timeAgo(version().published_at)}</p></>}</Show></Show></div></aside></div></Show>
        <Show when={tab() === 'architecture'}><section class="architecture-panel"><div class="section-heading"><div><h2>Architecture</h2><p>The organization’s silicon.yaml configuration.</p></div><button class="button" onClick={() => copy(item().yaml, 'Configuration')}>Copy YAML</button></div><Show when={item().yaml} fallback={<div class="empty panel">No architecture configuration is available.</div>}><Architecture yaml={item().yaml} /></Show></section></Show>
        <Show when={tab() === 'releases'}><section class="releases"><div class="section-heading"><div><h2>Releases</h2><p>Published versions and their source commits.</p></div></div><Show when={!versions.error} fallback={<ErrorMessage error={versions.error} retry={reloadVersions} />}><Show when={!versions.loading} fallback={<div class="loading">Loading releases…</div>}><For each={versions()} fallback={<div class="empty panel"><h2>No releases yet</h2><p>The organization can publish the first version with the CLI.</p></div>}>{version => <article class="release panel"><div class="release-title"><h3><Icon name="releases" />v{version.version}</h3><code title={version.commit}>{version.commit.slice(0, 10)}</code><time>{timeAgo(version.published_at)}</time></div><p>{version.notes || 'No release notes provided.'}</p><button class="button" onClick={() => archive(version.version)}><Icon name="download" size={16} />Download bundle</button></article>}</For></Show></Show></section></Show>
        <Show when={tab() === 'discussions'}><section class="discussions"><div class="section-heading"><div><h2>Discussions</h2><p>Questions and notes from the community.</p></div></div><Show when={!discussions.error} fallback={<ErrorMessage error={discussions.error} retry={reloadDiscussions} />}><Show when={!discussions.loading} fallback={<div class="loading">Loading discussions…</div>}><div class="comments"><For each={discussions()?.filter(item => !item.parent_id)} fallback={<div class="empty panel"><h2>Start a conversation</h2><p>No discussions yet.</p></div>}>{item => <CommentThread item={item} all={discussions() || []} />}</For></div></Show></Show><Show when={props.session.authenticated} fallback={<button class="button" onClick={props.login}>Log in to join the discussion</button>}><form class="comment-form" onSubmit={post}><label for="comment">Add a comment</label><textarea id="comment" required rows={4} value={comment()} onInput={event => setComment(event.currentTarget.value)} placeholder="Leave a note for the organization…" /><button class="button primary" disabled={posting() || !comment().trim()}>{posting() ? 'Posting…' : 'Post comment'}</button></form></Show></section></Show>
        <Show when={!['code', 'architecture', 'releases', 'discussions'].includes(tab())}><div class="empty panel"><h2>Page not found</h2><a href={repoUrl(props.id)}>View code</a></div></Show>
      </div>
    </>}</Show>
  </Show>;
}

function Explore(props: { session: Session; search: string }) {
  const initial = () => new URLSearchParams(props.search);
  const [query, setQuery] = createSignal(initial().get('q') || '');
  const [org, setOrg] = createSignal(initial().get('org') || '');
  const [sort, setSort] = createSignal('updated');
  const [mine, setMine] = createSignal(false);
  const [starters, { refetch }] = createResource(() => [query(), mine() ? props.session.org_id || '' : org()] as const, async ([q, organization]) => {
    const params = new URLSearchParams();
    if (q) params.set('q', q);
    if (organization) params.set('org', organization);
    const result = await api<Starter[] | { items: Starter[] }>(`/api/v1/starters?${params}`);
    return Array.isArray(result) ? result : result.items;
  });
  createEffect(() => { setOrg(initial().get('org') || ''); setQuery(initial().get('q') || ''); });
  const visible = () => starters.error ? [] : [...(starters() || [])].sort((a, b) => sort() === 'stars' ? b.stars - a.stars : sort() === 'downloads' ? b.downloads - a.downloads : +new Date(b.updated_at) - +new Date(a.updated_at));
  return <section class="page-width explore"><header class="explore-heading"><span class="eyebrow">THE SILICON REGISTRY</span><h1>Find your next starting point.</h1><p>Explore silicon architectures published by organizations.</p></header><div class="explore-tools"><div class="search"><Icon name="search" size={20} /><input aria-label="Search starters" type="search" value={query()} onInput={event => setQuery(event.currentTarget.value)} placeholder="Search starters, architectures, or tags…" /><kbd>⌘ K</kbd></div><select aria-label="Sort starters" value={sort()} onChange={event => setSort(event.currentTarget.value)}><option value="updated">Recently updated</option><option value="stars">Most starred</option><option value="downloads">Most downloaded</option></select></div><div class="registry-filters"><button class={!mine() && !org() ? 'active' : ''} onClick={() => { setMine(false); setOrg(''); }}>All starters</button><Show when={props.session.org_id}><button class={mine() ? 'active' : ''} onClick={() => setMine(true)}>My organization</button></Show><Show when={org() && !mine()}><span>Organization: {org()}</span></Show></div><Show when={!starters.error} fallback={<ErrorMessage error={`The registry could not be loaded. ${message(starters.error)}`} retry={refetch} />}><Show when={!starters.loading} fallback={<div class="loading">Loading the registry…</div>}><div class="starter-list"><For each={visible()} fallback={<div class="empty panel"><Icon name="book" size={30} /><h2>{query() || org() || mine() ? 'No matching starters' : 'No starters yet'}</h2><p>{query() || org() || mine() ? 'Try another search or organization.' : 'Organizations can add the first starter after logging in.'}</p></div>}>{item => <article class="starter-card panel"><div class="starter-card-heading"><Icon name="book" size={19} /><a href={repoUrl(item.id)}>{item.owner} <span>/</span> <strong>{item.name}</strong></a><span class="badge">{item.visibility}</span></div><p>{item.description || 'No description provided.'}</p><div class="tags"><For each={item.tags}>{tag => <a href={`/?q=${encodeURIComponent(tag)}`}>{tag}</a>}</For></div><div class="starter-card-footer"><span><Icon name="star" size={15} />{item.stars}</span><span><Icon name="download" size={15} />{item.downloads}</span><span>Updated {timeAgo(item.updated_at)}</span><a href={repoUrl(item.id)}>View code →</a></div></article>}</For></div></Show></Show></section>;
}

function CreateStarter(props: { session: Session; login: () => void; navigate: (path: string) => void }) {
  const [id, setId] = createSignal('');
  const organizations = () => props.session.org_ids?.length ? props.session.org_ids : props.session.org_id ? [props.session.org_id] : [];
  const [organization, setOrganization] = createSignal(props.session.org_id || organizations()[0] || '');
  const [name, setName] = createSignal('');
  const [description, setDescription] = createSignal('');
  const [yaml, setYaml] = createSignal('');
  const [visibility, setVisibility] = createSignal('public');
  const [error, setError] = createSignal('');
  const [saving, setSaving] = createSignal(false);
  const submit = async (event: SubmitEvent) => {
    event.preventDefault(); setSaving(true); setError('');
    try {
      const item = await api<Starter>('/api/v1/starters', { method: 'POST', body: JSON.stringify({ id: `${organization()}.${id().trim()}`, org_id: organization(), name: name().trim(), description: description().trim(), yaml: yaml(), visibility: visibility(), tags: [] }) });
      props.navigate(repoUrl(item.id));
    } catch (error) { setError(message(error)); }
    finally { setSaving(false); }
  };
  return <section class="page-width create-page"><a href="/">← Explore starters</a><h1>Create a starter</h1><p>Add your organization’s architecture. Use the CLI to push repository files and publish releases.</p><Show when={props.session.authenticated && organizations().length} fallback={<div class="notice"><p>{props.session.authenticated ? 'An attached organization is required to create a starter.' : 'Log in with an organization to create a starter.'}</p><button class="button primary" onClick={props.login}>{props.session.authenticated ? 'Connect an organization' : 'Log in with IAM'}</button></div>}><form onSubmit={submit} class="create-form"><label>Organization<select required value={organization()} onChange={event => setOrganization(event.currentTarget.value)}><For each={organizations()}>{org => <option value={org}>{org}</option>}</For></select></label><label>Starter ID<div class="starter-id-field"><span>{organization()}.</span><input required pattern="[A-Za-z0-9._-]+" value={id()} onInput={event => setId(event.currentTarget.value)} placeholder="starter-name" /></div></label><label>Name<input required value={name()} onInput={event => setName(event.currentTarget.value)} /></label><label>Description<input value={description()} onInput={event => setDescription(event.currentTarget.value)} /></label><label>Visibility<select value={visibility()} onChange={event => setVisibility(event.currentTarget.value)}><option value="public">Public</option><option value="private">Private</option></select></label><label>silicon.yaml<textarea required rows={13} value={yaml()} onInput={event => setYaml(event.currentTarget.value)} spellcheck={false} placeholder="Paste your silicon.yaml configuration" /></label><Show when={error()}><ErrorMessage error={error()} /></Show><div class="form-actions"><a class="button" href="/">Cancel</a><button class="button primary" disabled={saving()}>{saving() ? 'Creating…' : 'Create starter'}</button></div></form></Show></section>;
}
function Docs() {
  return <section class="page-width docs"><span class="eyebrow">FOR CARBONS & SILICONS</span><h1>Starter, in your terminal.</h1><p>Pull architectures, push repository changes, and publish releases with the Starter CLI.</p><div class="docs-grid"><article class="panel"><h2>1. Install</h2><p>Build the CLI from the project source.</p><pre><code>cargo install --path crates/cli</code></pre><a href="https://github.com/teamofsilicons/silicon-starter">Project source ↗</a></article><article class="panel"><h2>2. Pull</h2><p>Replace the ID with a starter from the registry.</p><pre><code>starter pull organization.starter</code></pre></article><article class="panel"><h2>3. Publish</h2><p>Commit and push your organization’s changes, then publish a version.</p><pre><code>{'starter commit "update architecture"\nstarter push\nstarter publish latest 1.0'}</code></pre></article></div></section>;
}
function App() {
  const [route, setRoute] = createSignal(parseRoute());
  const [search, setSearch] = createSignal(location.search);
  const [session, setSession] = createSignal<Session>({ authenticated: false });
  const [authLoading, setAuthLoading] = createSignal(true);
  const [authError, setAuthError] = createSignal('');
  const updateLocation = () => { setRoute(parseRoute()); setSearch(location.search); };
  const navigate = (path: string) => { history.pushState({}, '', path); updateLocation(); window.scrollTo(0, 0); };
  const login = () => { location.href = `${API}/auth/login?return_to=${encodeURIComponent(location.href)}`; };
  const accountName = () => {
    const actor = session().actor;
    return typeof actor === 'string' ? actor : actor?.display_name || actor?.name || (session().authenticated ? 'Signed in' : 'Guest');
  };
  onMount(() => {
    const click = (event: MouseEvent) => {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const anchor = (event.target as Element).closest<HTMLAnchorElement>('a[href]');
      if (!anchor || anchor.target || anchor.hasAttribute('download')) return;
      const url = new URL(anchor.href);
      if (url.origin !== location.origin || anchor.getAttribute('href')?.startsWith('#')) return;
      event.preventDefault(); navigate(url.pathname + url.search + url.hash);
    };
    const keyboard = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') { event.preventDefault(); document.querySelector<HTMLInputElement>('input[aria-label="Search starters"]')?.focus(); }
    };
    document.addEventListener('click', click); window.addEventListener('popstate', updateLocation); window.addEventListener('keydown', keyboard);
    onCleanup(() => { document.removeEventListener('click', click); window.removeEventListener('popstate', updateLocation); window.removeEventListener('keydown', keyboard); });
    void (async () => {
      try {
        const url = new URL(location.href);
        let slt = url.searchParams.get('slt') || '';
        if (!slt && url.hash.startsWith('#slts=')) {
          const entries = JSON.parse(decodeURIComponent(url.hash.slice(6))) as { app_id?: string; slt?: string }[];
          slt = entries.find(entry => entry.app_id === 'tos>starter')?.slt || '';
        }
        if (slt) {
          const state = url.searchParams.get('state') || undefined;
          url.searchParams.delete('slt'); url.searchParams.delete('state'); url.hash = '';
          history.replaceState({}, '', url.pathname === '/auth/callback' ? '/' : url.pathname + url.search); updateLocation();
          await api('/auth/callback', { method: 'POST', body: JSON.stringify({ slt, state }) });
        }
        setSession(await api<Session>('/auth/session'));
      } catch (error) { setAuthError(`Could not verify your session. ${message(error)}`); }
      finally { setAuthLoading(false); }
    })();
  });
  return <div class="app"><header class="site-header"><a class="brand" href="/"><span class="brand-mark">✦</span>starter</a><nav aria-label="Main navigation"><a href="/" aria-current={route().page === 'explore' ? 'page' : undefined}>Explore</a><a href="/docs" aria-current={route().page === 'docs' ? 'page' : undefined}>Docs</a></nav><div class="account"><Show when={!authLoading()} fallback={<span class="muted">Checking session…</span>}><Show when={session().authenticated} fallback={<button class="button" onClick={login}>Log in</button>}><span class="account-name" title={session().org_id || undefined}>{accountName()}</span></Show><a class="button primary" href="/new"><Icon name="plus" size={16} />New starter</a></Show></div></header>
    <Show when={authError()}><div class="page-width"><ErrorMessage error={authError()} /></div></Show>
    <main><Show when={route().page === 'starters' && route().id} fallback={<Show when={route().page === 'explore'} fallback={<Show when={route().page === 'docs'} fallback={<Show when={route().page === 'new'} fallback={<div class="page-width empty"><h1>Page not found</h1><a href="/">Explore starters</a></div>}><Show when={!authLoading()} fallback={<div class="page-width loading">Checking your organization…</div>}><CreateStarter session={session()} login={login} navigate={navigate} /></Show></Show>}><Docs /></Show>}><Explore session={session()} search={search()} /></Show>}><RepositoryPage id={route().id} route={route()} session={session()} navigate={navigate} login={login} /></Show></main>
    <footer class="site-footer"><span>✦ Silicon Starter</span><a href="/docs">Documentation</a><a href="https://github.com/teamofsilicons/silicon-starter">GitHub ↗</a></footer>
  </div>;
}
render(() => <App />, document.getElementById('root')!);
