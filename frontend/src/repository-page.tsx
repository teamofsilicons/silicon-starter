'use client';

import { Fragment, useEffect, useMemo, useState, type FormEvent, type ReactNode } from 'react';
import Link from 'next/link';
import { Button, Input, Card, Textarea } from './silicon-ui';
import type { Session } from './api';
import {
  api, downloadFile, ErrorMessage, Icon, message, repoUrl, sizeLabel, timeAgo, useResource,
  type Discussion, type Repository, type RepoFile, type Route, type Starter, type Version,
} from './shared';

function CommentThread({ item, all, ancestors = [] }: { item: Discussion; all: Discussion[]; ancestors?: string[] }) {
  const branch = [...ancestors, item.id];
  return <article className="comment">
    <div className="comment-heading"><span className="avatar">{item.author.slice(0, 1).toUpperCase()}</span><strong>{item.author}</strong><time title={item.created_at}>{timeAgo(item.created_at)}</time></div>
    <p>{item.body}</p>
    <div className="replies">{all.filter(child => child.parent_id === item.id && !branch.includes(child.id)).map(child => <CommentThread key={child.id} item={child} all={all} ancestors={branch} />)}</div>
  </article>;
}

// shortcut: render common README blocks as safe JSX; use a Markdown parser when full CommonMark is needed.
export function Readme({ content, link: resolveLink }: { content: string; link: (path: string) => string }) {
  const inline = (text: string): ReactNode => text.split(/(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\([^\s)]+\))/g).map((part, index) => {
    if (part.startsWith('`')) return <code key={index}>{part.slice(1, -1)}</code>;
    if (part.startsWith('**')) return <strong key={index}>{part.slice(2, -2)}</strong>;
    const link = /^\[([^\]]+)\]\(([^\s)]+)\)$/.exec(part);
    if (link) {
      const href = /^(https?:|mailto:)/i.test(link[2]) ? link[2] : /^[a-z][a-z\d+.-]*:|^\/\//i.test(link[2]) ? '' : resolveLink(link[2]);
      return href ? <a key={index} href={href}>{link[1]}</a> : link[1];
    }
    return part;
  });
  const lines = content.split('\n');
  const nodes: ReactNode[] = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const key = i;
    if (/^\s*```/.test(line)) {
      const code: string[] = [];
      while (++i < lines.length && !/^\s*```/.test(lines[i])) code.push(lines[i]);
      nodes.push(<pre key={key}><code>{code.join('\n')}</code></pre>);
    } else if (/^###\s/.test(line)) nodes.push(<h3 key={key}>{inline(line.slice(4))}</h3>);
    else if (/^##\s/.test(line)) nodes.push(<h2 key={key}>{inline(line.slice(3))}</h2>);
    else if (/^#\s/.test(line)) nodes.push(<h1 key={key}>{inline(line.slice(2))}</h1>);
    else if (/^[-*]\s/.test(line)) {
      const items = [line.slice(2)];
      while (i + 1 < lines.length && /^[-*]\s/.test(lines[i + 1])) items.push(lines[++i].slice(2));
      nodes.push(<ul key={key}>{items.map((item, index) => <li key={index}>{inline(item)}</li>)}</ul>);
    } else if (line.trim()) nodes.push(<p key={key}>{inline(line)}</p>);
  }
  return <div className="markdown">{nodes}</div>;
}

function Architecture({ yaml }: { yaml: string }) {
  const blocks = useMemo(() => {
    const sections: { name: string; body: string }[] = [];
    for (const line of yaml.split('\n')) {
      const section = /^(silicon|isi|access|flow):/.exec(line);
      if (section || !sections.length) sections.push({ name: section?.[1] || 'configuration', body: line });
      else sections[sections.length - 1].body += `\n${line}`;
    }
    return sections.filter(block => block.body.trim());
  }, [yaml]);
  return <div className="architecture-blocks">{blocks.map((block, index) => <section key={index} className={`architecture-block panel ${block.name}`}><header><span></span>{block.name}</header><pre className="architecture-source"><code>{block.body}</code></pre></section>)}</div>;
}

function Template({ id, repository }: { id: string; repository: Repository }) {
  const source = (path = '') => repoUrl(id, path ? 'blob' : 'tree', `.starterbase${path ? `/${path}` : ''}`);
  const value = (value: unknown) => typeof value === 'string' && value ? value : JSON.stringify(value, null, 2);
  const ingredients = repository.files.filter(file => file.path.startsWith('.starterbase/'));
  const recipe = repository.template;
  return <section className="template-panel">
    <div className="section-heading"><div><h2>{recipe?.name || 'Template'}</h2><p>{recipe?.description || 'Questions, build steps, and source ingredients for this starter.'}</p></div><Link className="button" href={source('starter.yaml')}>View starter.yaml</Link></div>
    {repository.template_error && <ErrorMessage error={`Template preview unavailable: ${repository.template_error}`} />}
    {!recipe && !repository.template_error && <div className="notice">Template metadata is unavailable. Browse the source ingredients below.</div>}
    {recipe && <>
      <p className="template-intro">This is a preview of the author’s recipe. Expressions and scripts run locally through the CLI. Downloads auto-update by default; pinned downloads and developer pulls do not.</p>
      <ol className="template-flow">
        <li><section className="panel template-stage"><header><span>1</span><h3>Choose a folder</h3></header><p>Starter asks for an available destination folder unless you supply one. Your folder name is separate from the silicon’s identity.</p></section></li>
        <li><section className="panel template-stage"><header><span>2</span><h3>Resolve answers in order</h3></header><p>Press Enter to accept a resolved default. Saved answers are reused on updates; variables without a prompt resolve silently.</p>
          <ol className="template-questions">{recipe.variables.length ? recipe.variables.map(variable => <li key={variable.name} className="template-question" data-variable={variable.name}>
            <div className="question-heading"><code>{variable.name}</code><span className="badge">{variable.type}</span>{!variable.prompt && <span className="muted">No question · resolves silently</span>}</div>
            {variable.prompt && <h4>{variable.prompt}</h4>}
            <dl>{variable.when && <div><dt>Active when</dt><dd><pre><code>{variable.when}</code></pre></dd></div>}{variable.choices != null && <div><dt>Choices</dt><dd><pre><code>{value(variable.choices)}</code></pre></dd></div>}<div><dt>Default</dt><dd><pre><code>{value(variable.default)}</code></pre></dd></div></dl>
            {variable.type === 'secret' && <p className="muted">Input is masked in the terminal. The answer is saved locally and available to templates and scripts.</p>}
          </li>) : <li className="muted">This template has no variables.</li>}</ol>
        </section></li>
        <li><section className="panel template-stage"><header><span>3</span><h3>Render files</h3></header><p>Templates use the active answers. Other declared files copy unchanged into a fresh build directory.</p><ul className="template-files">{recipe.files.length ? recipe.files.map((file, index) => <li key={index}><Link href={source(file.from)}><code>{file.from}</code></Link><span aria-label="produces">→</span><code>{file.to}</code></li>) : <li className="muted">No declared files; build scripts may create outputs.</li>}</ul></section></li>
        <li><section className="panel template-stage"><header><span>4</span><h3>Run build steps</h3></header><p>Commands run in the recipe directory after rendering. Script output in <code>STARTER_OUTPUT</code> is included in the build.</p><ol className="template-build">{recipe.build.length ? recipe.build.map((command, index) => <li key={index}><pre><code>{command}</code></pre></li>) : <li className="muted">No build commands.</li>}</ol></section></li>
        <li><section className="panel template-stage"><header><span>5</span><h3>Validate and apply</h3></header><p>Starter validates the generated configuration and merges it with local edits. Answers and the generated baseline are saved after the complete build succeeds. An unresolved update leaves live files intact.</p></section></li>
      </ol>
    </>}
    <section className="panel template-ingredients"><header><h3>Source ingredients</h3><Link href={source()}>Browse .starterbase</Link></header>{repository.truncated && <p className="notice">This preview is incomplete. Download the Git bundle to see all source ingredients.</p>}{ingredients.map(file => <Link key={file.path} className="file-row" href={repoUrl(id, 'blob', file.path)}><Icon name="file" /><span>{file.path.slice('.starterbase/'.length)}</span><small>{sizeLabel(file)}</small></Link>)}</section>
  </section>;
}

export function RepositoryPage(props: { id: string; route: Route; session: Session; navigate: (path: string) => void; login: () => void }) {
  const base = `/api/v1/starters/${encodeURIComponent(props.id)}`;
  const starter = useResource<Starter>(base);
  const repository = useResource<Repository>(`${base}/files`);
  const versions = useResource<Version[]>(`${base}/versions`);
  const discussions = useResource<Discussion[]>(props.route.tab === 'discussions' ? `${base}/discussions` : false);
  const [notice, setNotice] = useState('');
  const [actionError, setActionError] = useState('');
  const [comment, setComment] = useState('');
  const [posting, setPosting] = useState(false);
  const [filter, setFilter] = useState('');
  const files = repository.error ? [] : repository.data?.files || [];
  const releases = useMemo(() => [...(versions.data || [])].sort((a, b) => +new Date(b.published_at) - +new Date(a.published_at)), [versions.data]);
  const hasTemplate = files.some(file => file.path === '.starterbase/starter.yaml');
  const tab = ['code', 'tree', 'blob'].includes(props.route.tab) ? 'code' : props.route.tab;
  const folder = props.route.tab === 'tree' ? props.route.path : '';
  const selected = props.route.tab === 'blob' ? files.find(file => file.path === props.route.path) : undefined;
  const prefix = folder ? `${folder}/` : '';
  const directoryEntries = new Map<string, { name: string; path: string; directory: boolean; file?: RepoFile }>();
  for (const file of files) {
    if (!file.path.startsWith(prefix)) continue;
    const remainder = file.path.slice(prefix.length);
    const name = remainder.split('/')[0];
    if (!name) continue;
    directoryEntries.set(name, { name, path: prefix + name, directory: remainder.includes('/'), file: remainder.includes('/') ? undefined : file });
  }
  const entries = [...directoryEntries.values()].filter(entry => entry.name.toLowerCase().includes(filter.toLowerCase())).sort((a, b) => Number(b.directory) - Number(a.directory) || a.name.localeCompare(b.name));
  const breadcrumbs = props.route.path.split('/').filter(Boolean);
  const readme = files.find(file => file.path.slice(0, file.path.lastIndexOf('/') + 1) === prefix && /^readme(?:\.md|\.markdown|\.txt)?$/i.test(file.path.split('/').pop() || ''));
  useEffect(() => { setFilter(''); setNotice(''); setActionError(''); }, [props.id, props.route.path, props.route.tab]);
  const copy = async (text: string, label: string) => {
    try { await navigator.clipboard.writeText(text); setNotice(`${label} copied.`); }
    catch { setActionError('Clipboard access was denied. Select and copy the text directly.'); }
  };
  const archive = async (ref?: string) => {
    try {
      setActionError('');
      const payload = await api<{ bundle_base64: string }>(`${base}/archive${ref ? `?ref=${encodeURIComponent(ref)}` : ''}`);
      const bytes = Uint8Array.from(atob(payload.bundle_base64), character => character.charCodeAt(0));
      downloadFile(bytes, `${props.id}.bundle`, 'application/octet-stream');
    } catch (error) { setActionError(message(error)); }
  };
  const star = async () => {
    if (!props.session.authenticated) { props.login(); return; }
    try { starter.mutate(await api<Starter>(`${base}/star`, { method: 'POST' })); setNotice('Starter starred.'); }
    catch (error) { setActionError(message(error)); }
  };
  const post = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!comment.trim() || posting) return;
    setPosting(true); setActionError('');
    try { await api(`${base}/discussions`, { method: 'POST', body: JSON.stringify({ body: comment.trim() }) }); setComment(''); await discussions.refetch(); }
    catch (error) { setActionError(message(error)); }
    finally { setPosting(false); }
  };
  const readmeLink = (path: string) => {
    if (path.startsWith('#')) return path;
    const normalized = new URL(path, `https://repository.invalid/${prefix}`).pathname.slice(1);
    return repoUrl(props.id, files.some(file => file.path === normalized) ? 'blob' : 'tree', normalized);
  };
  const item = starter.data;
  if (starter.error) return <section className="page-width"><Link href="/">← Explore starters</Link><ErrorMessage error={starter.error} retry={starter.refetch} /></section>;
  if (!item) return <div className="page-width loading">Loading starter…</div>;
  const latestRelease = releases[0];
  return <>
    <section className="repository-heading">
      <div className="page-width repository-title"><div className="repository-name"><Icon name="book" size={23} /><Link href={`/?owner=${encodeURIComponent(item.owner)}`}>{item.owner}</Link><span>/</span><h1><Link href={repoUrl(item.id)}>{item.name}</Link></h1><span className="badge">{item.visibility}</span></div><Button variant="secondary" className="button" onClick={star}><Icon name="star" size={16} /> Star <span className="count">{item.stars}</span></Button></div>
      <nav className="page-width repository-tabs" aria-label="Starter sections">{[['code', 'Code'], ...(hasTemplate ? [['template', 'Template']] : []), ['architecture', 'Architecture'], ['releases', 'Releases'], ['discussions', 'Discussions']].map(([name, label]) => <Link key={name} href={repoUrl(props.id, name)} className={tab === name ? 'active' : ''} aria-current={tab === name ? 'page' : undefined}><Icon name={name} />{label}{name === 'releases' && !versions.error && releases.length > 0 && <span className="count">{releases.length}</span>}</Link>)}</nav>
    </section>
    <div className="page-width repository-body">
      {notice && <div className="notice" role="status">{notice}</div>}
      {actionError && <ErrorMessage error={actionError} />}
      {tab === 'code' && <div className="repository-layout"><div className="repository-main">
        <div className="code-toolbar"><span className="revision"><Icon name="code" size={16} />{repository.error ? 'Source' : repository.data?.draft ? 'Draft' : 'Latest source'}</span>{!versions.error && <Link href={repoUrl(props.id, 'releases')} className="release-link"><Icon name="releases" size={16} />{releases.length} releases</Link>}<details className="code-menu"><summary className="button primary"><Icon name="code" size={16} /> Code <span>⌄</span></summary><div className="code-menu-content"><strong>Use this starter</strong><div className="copy-command"><Input label="Pull with the CLI" id="pull-command" readOnly value={`starter pull ${props.id}`} /><Button variant="secondary" className="button" aria-label="Copy pull command" onClick={() => copy(`starter pull ${props.id}`, 'CLI command')}><Icon name="copy" size={15} /></Button></div><Button variant="secondary" className="button full-width" onClick={() => archive()}><Icon name="download" size={16} />Download Git bundle</Button></div></details></div>
        {props.route.path && <nav className="file-breadcrumbs" aria-label="File path"><Link href={repoUrl(props.id)}>{props.id}</Link>{breadcrumbs.map((part, index) => <Fragment key={index}><span>/</span>{index < breadcrumbs.length - 1 ? <Link href={repoUrl(props.id, 'tree', breadcrumbs.slice(0, index + 1).join('/'))}>{part}</Link> : <strong>{part}</strong>}</Fragment>)}</nav>}
        {repository.error ? <ErrorMessage error={repository.error} retry={repository.refetch} /> : repository.loading ? <div className="loading panel">Reading repository files…</div> : <>
          {repository.data?.truncated && <div className="notice">This repository is too large to preview completely. Download the Git bundle for all files.</div>}
          {props.route.tab === 'blob' ? selected ? <section className="file-view panel">
            <header><div><strong>{selected.path.split('/').pop()}</strong><span>{sizeLabel(selected)}</span>{selected.content !== null && <span>{selected.content.split('\n').length} lines</span>}</div>{selected.content !== null && <Button variant="secondary" className="button" onClick={() => copy(selected.content!, 'File')}>Copy</Button>}</header>
            {selected.content !== null ? <div className="code-scroll"><pre className="source-code">{selected.content.split('\n').map((line, index) => <div key={index} className="source-line" id={`L${index + 1}`}><a className="line-number" href={`#L${index + 1}`} aria-label={`Line ${index + 1}`}>{index + 1}</a><code>{line || '\n'}</code></div>)}</pre></div> : <div className="empty"><h2>Preview unavailable</h2><p>{({ binary: 'Binary file. Download the Git bundle to open it.', large: 'This file is too large to preview. Download the Git bundle to open it.', response_limit: 'The repository preview size limit was reached. Download the Git bundle to open this file.', symlink: 'Symbolic link. Download the Git bundle to inspect its target.', submodule: 'Git submodule. Download the Git bundle to inspect its reference.' } as Record<string, string>)[selected.reason || ''] || 'This file cannot be displayed as text. Download the Git bundle to open it.'}</p></div>}
          </section> : <div className="empty panel"><h2>File not found</h2><Link href={repoUrl(props.id)}>Return to repository root</Link></div> : !folder || files.some(file => file.path.startsWith(prefix)) ? <>
            <div className="file-table panel"><div className="source-heading"><strong>{repository.data?.draft ? 'Account draft' : 'Repository files'}</strong>{repository.data?.commit && <code title={repository.data.commit}>{repository.data.commit.slice(0, 10)}</code>}<span>{files.length} files</span></div>
              {files.length > 12 && <div className="file-filter"><Icon name="search" size={16} /><Input label="Filter this directory" placeholder="Filter this directory…" value={filter} onChange={event => setFilter(event.currentTarget.value)} /></div>}
              {folder && <Link className="file-row parent-folder" href={repoUrl(props.id, folder.includes('/') ? 'tree' : 'code', folder.split('/').slice(0, -1).join('/'))}><Icon name="folder" />..</Link>}
              {entries.length ? entries.map(entry => <Link key={entry.path} className="file-row" href={repoUrl(props.id, entry.directory ? 'tree' : 'blob', entry.path)}><Icon name={entry.directory ? 'folder' : 'file'} /><span>{entry.name}</span><small>{entry.directory ? 'Directory' : entry.file && sizeLabel(entry.file)}</small></Link>) : <div className="empty compact">{filter ? 'No files match this filter.' : 'No source files are available.'}</div>}
            </div>
            {readme?.content != null && <section className="readme panel"><header><Icon name="book" size={16} /><Link href={repoUrl(props.id, 'blob', readme.path)}>{readme.path.split('/').pop()}</Link></header><Readme content={readme.content} link={readmeLink} /></section>}
          </> : <div className="empty panel"><h2>Directory not found</h2><Link href={repoUrl(props.id)}>Return to repository root</Link></div>}
        </>}
      </div><aside className="repository-about">
        <h2>About</h2><p>{item.description || 'No description provided.'}</p><div className="tags">{item.tags.map(tag => <Link key={tag} href={`/?q=${encodeURIComponent(tag)}`}>{tag}</Link>)}</div>
        <dl><div><dt>Published by</dt><dd><Link href={`/?owner=${encodeURIComponent(item.owner)}`}>{item.owner}</Link></dd></div><div><dt>Starter ID</dt><dd><code>{item.id}</code></dd></div></dl>
        <p className="about-stat"><Icon name="star" size={16} />{item.stars} stars</p><p className="about-stat"><Icon name="download" size={16} />{item.downloads.toLocaleString()} downloads</p><p className="muted">Updated {timeAgo(item.updated_at)}</p>
        <div className="about-releases"><h2><Link href={repoUrl(props.id, 'releases')}>Releases</Link>{!versions.error && <span className="count">{releases.length}</span>}</h2>{versions.error ? <p className="muted">Release history unavailable.</p> : latestRelease ? <><Link className="latest-release" href={repoUrl(props.id, 'releases')}><Icon name="releases" size={17} />v{latestRelease.version}<span className="latest-badge">Latest</span></Link><p className="muted">{timeAgo(latestRelease.published_at)}</p></> : <p className="muted">No releases published.</p>}</div>
      </aside></div>}
      {tab === 'template' && (repository.error ? <ErrorMessage error={repository.error} retry={repository.refetch} /> : repository.loading ? <div className="loading panel">Reading template…</div> : hasTemplate && repository.data ? <Template id={props.id} repository={repository.data} /> : <div className="empty panel"><h2>No template recipe</h2><p>{repository.data?.truncated ? 'The repository preview is incomplete. Download the Git bundle to check for a recipe.' : 'This starter uses its files directly, without a .starterbase/starter.yaml recipe.'}</p><Link href={repoUrl(props.id)}>Browse repository files</Link></div>)}
      {tab === 'architecture' && <section className="architecture-panel"><div className="section-heading"><div><h2>Architecture</h2><p>The starter’s silicon.yaml configuration.</p></div><Button variant="secondary" className="button" onClick={() => copy(item.yaml, 'Configuration')}>Copy YAML</Button></div>{item.yaml ? <Architecture yaml={item.yaml} /> : <div className="empty panel">No architecture configuration is available.</div>}</section>}
      {tab === 'releases' && <section className="releases"><div className="section-heading"><div><h2>Releases</h2><p>Published versions and their source commits.</p></div></div>{versions.error ? <ErrorMessage error={versions.error} retry={versions.refetch} /> : versions.loading ? <div className="loading">Loading releases…</div> : releases.length ? releases.map(version => <Card key={version.version} title={`v${version.version}`} className="release panel" action={<Button variant="secondary" className="button" onClick={() => archive(version.version)}><Icon name="download" size={16} />Download bundle</Button>}><div className="release-title"><Icon name="releases" /><code title={version.commit}>{version.commit.slice(0, 10)}</code><time>{timeAgo(version.published_at)}</time></div><p>{version.notes || 'No release notes provided.'}</p></Card>) : <div className="empty panel"><h2>No releases yet</h2><p>The owner can publish the first version with the CLI.</p></div>}</section>}
      {tab === 'discussions' && <section className="discussions"><div className="section-heading"><div><h2>Discussions</h2><p>Questions and notes from the community.</p></div></div>
        {discussions.error ? <ErrorMessage error={discussions.error} retry={discussions.refetch} /> : discussions.loading ? <div className="loading">Loading discussions…</div> : <div className="comments">{discussions.data?.some(comment => !comment.parent_id) ? discussions.data.filter(comment => !comment.parent_id).map(comment => <CommentThread key={comment.id} item={comment} all={discussions.data || []} />) : <div className="empty panel"><h2>Start a conversation</h2><p>No discussions yet.</p></div>}</div>}
        {props.session.authenticated ? <form className="comment-form" onSubmit={post}><Textarea label="Add a comment" id="comment" required rows={4} value={comment} onChange={event => setComment(event.currentTarget.value)} placeholder="Leave a note for the author…" /><Button type="submit" variant="primary" className="button primary" disabled={posting || !comment.trim()}>{posting ? 'Posting…' : 'Post comment'}</Button></form> : <Button variant="secondary" className="button" onClick={props.login}>Log in to join the discussion</Button>}
      </section>}
      {!['code', 'template', 'architecture', 'releases', 'discussions'].includes(tab) && <div className="empty panel"><h2>Page not found</h2><Link href={repoUrl(props.id)}>View code</Link></div>}
    </div>
  </>;
}
