'use client';

import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import Link from 'next/link';
import { Badge, Button, Card, Input, Select, Textarea } from './silicon-ui';
import type { Session } from './api';
import { blockDraftKey, blockDraftStore, type BlockDraft } from './block-draft';
import { Readme } from './repository-page';
import {
  API, api, message, repoUrl, blockUrl, blockLabel, downloadFile, timeAgo,
  Icon, ErrorMessage, useResource, type Starter, type Block, type BlockContent,
} from './shared';

export function BlockList(props: { query: string; kind?: string; embedded?: boolean; items?: Block[] }) {
  const blocks = useResource<(Starter | Block)[]>(props.items !== undefined ? false : props.query
    ? `/api/v1/search?${new URLSearchParams({ q: props.query })}` : '/api/v1/blocks');
  const visible = useMemo(() => (props.items ?? blocks.data ?? [])
    .filter((item): item is Block => 'kind' in item)
    .filter(item => !props.kind || item.kind === props.kind), [props.items, props.kind, blocks.data]);
  const show = !props.embedded || !!props.items?.length || !!blocks.error || !!blocks.data?.length;
  return <section className="block-list">{show && <>
    {props.embedded && <div className="section-heading"><div><h2>Genes, ISIs &amp; functions</h2><p>Reusable blocks for your silicon.</p></div><Link href={`/blocks?q=${encodeURIComponent(props.query)}`}>Browse all blocks →</Link></div>}
    {blocks.error ? <ErrorMessage error={blocks.error} retry={blocks.refetch} /> : blocks.loading ? <div className="loading">Loading blocks…</div> : <div className="starter-list">
      {visible.length ? visible.map(item => <article className="starter-card panel" key={item.id}>
        <div className="starter-card-heading"><Icon name={item.kind === 'gene' ? 'book' : 'code'} /><Link href={blockUrl(item.id)}><strong>{item.id}</strong></Link><Badge size="sm">{blockLabel(item.kind)}</Badge><Badge size="sm">{item.visibility}</Badge></div>
        <p>{item.description || item.name}</p><div className="starter-card-footer"><span>{item.owner}</span><span><Icon name="download" size={15} />{item.downloads}</span><span>Updated {timeAgo(item.updated_at)}</span><Link href={blockUrl(item.id)}>View block →</Link></div>
      </article>) : <div className="empty panel"><h2>No matching blocks</h2><p>Try another search, or publish a reusable block.</p></div>}
    </div>}
  </>}</section>;
}

export function Blocks(props: { search: string }) {
  const [query, setQuery] = useState(() => new URLSearchParams(props.search).get('q') || '');
  const [kind, setKind] = useState('');
  useEffect(() => setQuery(new URLSearchParams(props.search).get('q') || ''), [props.search]);
  return <section className="page-width explore">
    <header className="explore-heading"><span className="eyebrow">REUSABLE SILICON BLOCKS</span><h1>Genes, ISIs &amp; functions.</h1><p>Find prompts, internal silicons, and functions to use in your silicon.</p></header>
    <div className="explore-tools"><div className="search"><Icon name="search" size={20} /><Input label="Search blocks" type="search" value={query} onChange={event => setQuery(event.currentTarget.value)} placeholder="Search genes, ISIs, and functions…" /></div><Link className="button primary" href="/blocks/new"><Icon name="plus" size={16} />Publish a block</Link></div>
    <div className="registry-filters">{['', 'gene', 'isi', 'function'].map(value => <Button variant="secondary" key={value} aria-pressed={kind === value} className={kind === value ? 'active' : ''} onClick={() => setKind(value)}>{value ? blockLabel(value) : 'All blocks'}</Button>)}</div>
    <BlockList query={query} kind={kind} />
  </section>;
}

export function BlockPage(props: { id: string; session: Session }) {
  return <BlockDetail key={props.id} {...props} />;
}

function BlockDetail(props: { id: string; session: Session }) {
  const [version, setVersion] = useState('latest');
  const [error, setError] = useState('');
  const [downloading, setDownloading] = useState(false);
  const block = useResource<Block>(`/api/v1/blocks/${encodeURIComponent(props.id)}`);
  const versions = useResource<{ version: string; published_at: string }[]>(`/api/v1/blocks/${encodeURIComponent(props.id)}/versions`);
  const content = useResource<BlockContent>(!block.error && block.data?.kind === 'gene'
    ? `/api/v1/blocks/${encodeURIComponent(props.id)}/content?version=${encodeURIComponent(version)}` : false);
  const download = async () => {
    setError(''); setDownloading(true);
    try {
      const value = await api<BlockContent>(`/api/v1/blocks/${encodeURIComponent(props.id)}/download?version=${encodeURIComponent(version)}`);
      const name = `${props.id.replace(':', '-')}-${value.version.slice(0, 12)}`;
      if (value.kind === 'gene' && typeof value.text === 'string') downloadFile(value.text, `${name}.md`, 'text/markdown;charset=utf-8');
      else if (value.archive_base64) downloadFile(Uint8Array.from(atob(value.archive_base64), character => character.charCodeAt(0)), `${name}.zip`, 'application/zip');
      else throw new Error('The server returned no block content.');
    } catch (error) { setError(message(error)); }
    finally { setDownloading(false); }
  };
  const item = block.data;
  const canPublish = props.session.authenticated && !!props.session.actor && props.session.actor.uuid === item?.owner_uuid;
  return <section className="page-width block-page"><Link href="/blocks">← Explore blocks</Link>
    {block.error ? <ErrorMessage error={block.error} retry={block.refetch} /> : !item ? <div className="loading">Loading block…</div> : <>
      <div className="section-heading"><div><h1>{item.id}</h1><p>{item.description || item.name}</p><Badge size="sm">{blockLabel(item.kind)}</Badge> <Badge size="sm">{item.visibility}</Badge> <span className="muted">Published by {item.owner}</span></div>{canPublish && <Link className="button" href={`${blockUrl(props.id)}/edit`}>{item.kind === 'gene' ? 'Edit gene' : 'Publish a version'}</Link>}</div>
      <div className="block-toolbar"><Select label="Version" id="block-version" value={version} onValueChange={setVersion} options={[{ value: 'latest', label: `Latest · ${item.version.slice(0, 12)}` }, ...(!versions.error ? versions.data ?? [] : []).map(entry => ({ value: entry.version, label: `${entry.version.slice(0, 12)} · ${new Date(entry.published_at).toLocaleDateString()}` }))]} /><Button variant="primary" disabled={downloading} onClick={download}><Icon name="download" size={16} />{downloading ? 'Downloading…' : `Download ${item.kind === 'gene' ? 'Markdown' : 'ZIP'}`}</Button></div>
      {!!versions.error && <ErrorMessage error={versions.error} retry={versions.refetch} />}{error && <ErrorMessage error={error} />}
      <p className="muted block-version">Content hash: <code>{version === 'latest' ? item.version : version}</code></p>
      {item.kind === 'gene' ? content.error ? <ErrorMessage error={content.error} retry={content.refetch} /> : content.loading || !content.data ? <div className="loading">Loading gene…</div> : <section className="readme panel"><header><Icon name="book" size={16} />{item.id}.md</header><Readme content={content.data.text || ''} link={path => path.startsWith('#') ? path : ''} /></section> : <div className="notice">The ZIP contains <code>{item.kind === 'isi' ? 'isi.yaml' : 'function.yaml'}</code> and any supporting files. Each archive defines exactly one {blockLabel(item.kind)}.</div>}
      <p className="muted">Downloads use the latest content unless you select a version. Each published content hash remains available.</p><pre className="block-command"><code>{`starter download ${props.id}${version === 'latest' ? '' : `@${version}`}`}</code></pre>
    </>}
  </section>;
}

type PublishBlockProps = { id?: string; session: Session; login: () => void; navigate: (path: string) => void };

export function PublishBlock(props: PublishBlockProps) {
  const draftKey = blockDraftKey(API, props.session.context_id || 'anonymous', props.id);
  return <BlockEditor key={draftKey} {...props} draftKey={draftKey} />;
}

function BlockEditor(props: PublishBlockProps & { draftKey: string }) {
  const account = props.session.actor;
  const [kind, setKind] = useState<Block['kind']>('gene');
  const [id, setId] = useState('');
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [visibility, setVisibility] = useState('public');
  const [text, setText] = useState('');
  const [archive, setArchive] = useState<File>();
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const [pending, setPending] = useState<BlockDraft>();
  const [restoring, setRestoring] = useState(true);
  const active = useRef(false);
  const submitting = useRef(false);
  const existing = useResource<Block>(props.id ? `/api/v1/blocks/${encodeURIComponent(props.id)}` : false);
  const existingContent = useResource<BlockContent>(props.id && !existing.error && existing.data?.kind === 'gene'
    ? `/api/v1/blocks/${encodeURIComponent(props.id)}/content` : false);

  useEffect(() => {
    active.current = true;
    let cancelled = false;
    void blockDraftStore(props.draftKey, 'read').then(draft => {
      if (cancelled) return;
      if (draft) {
        setPending(draft);
        const body = draft.body, identifier = String(body.id).split(':');
        setKind(identifier[0] as Block['kind']); setId(identifier[1]);
        setName(String(body.name || '')); setDescription(String(body.description || ''));
        setVisibility(String(body.visibility)); setText(String(body.text || ''));
      }
      setRestoring(false);
    }).catch(error => { if (!cancelled) setError(message(error)); });
    return () => { cancelled = true; active.current = false; };
  }, [props.draftKey]);

  useEffect(() => {
    const current = existing.data;
    if (restoring || pending || existing.error || !current || existingContent.error) return;
    if (current.kind === 'gene' && !existingContent.data) return;
    setKind(current.kind); setId(current.id.split(':')[1]);
    setName(current.name); setDescription(current.description); setVisibility(current.visibility);
    setText(existingContent.data?.text || '');
  }, [existing.data, existing.error, existingContent.data, existingContent.error, pending, restoring]);

  const discard = async () => {
    try { await blockDraftStore(props.draftKey, 'remove'); if (active.current) { setPending(undefined); setError(''); } }
    catch (error) { if (active.current) setError(message(error)); }
  };
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (submitting.current || restoring) return;
    if (!props.session.authenticated || !account) { props.login(); return; }
    if (props.id && existing.data?.owner_uuid !== account.uuid) return;
    submitting.current = true; setSaving(true); setError('');
    try {
      let draft = pending;
      if (!draft) {
        let archive_base64: string | undefined;
        if (kind !== 'gene') {
          if (!archive || !archive.name.toLowerCase().endsWith('.zip')) throw new Error('Choose a ZIP file to publish.');
          if (archive.size > 8 * 1024 * 1024) throw new Error('The ZIP must be 8 MiB or smaller.');
          archive_base64 = await new Promise<string>((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => resolve(String(reader.result).split(',')[1]);
            reader.onerror = () => reject(new Error('Could not read the ZIP file.'));
            reader.readAsDataURL(archive);
          });
        } else if (new TextEncoder().encode(text).length > 1024 * 1024) throw new Error('The gene must be 1 MiB or smaller.');
        draft = { key: crypto.randomUUID(), body: { id: `${kind}:${id.trim()}`, name: name.trim(), description: description.trim(), visibility, ...(kind === 'gene' ? { text } : { archive_base64 }) } };
        await blockDraftStore(props.draftKey, 'write', draft);
        if (!active.current) return;
        setPending(draft);
      }
      if (!active.current) return;
      const item = await api<Block>('/api/v1/blocks', { method: 'POST', headers: { 'Idempotency-Key': draft.key }, body: JSON.stringify(draft.body) });
      await blockDraftStore(props.draftKey, 'remove');
      if (active.current) props.navigate(blockUrl(item.id));
    } catch (error) { if (active.current) setError(message(error)); }
    finally { submitting.current = false; if (active.current) setSaving(false); }
  };
  const loadError = existing.error || existingContent.error;
  const loading = !!props.id && (existing.loading || !existing.data || (existing.data.kind === 'gene' && (existingContent.loading || !existingContent.data)));
  const reload = () => { void existing.refetch(); void existingContent.refetch(); };
  return <section className="page-width create-page">
    <Link href={props.id ? blockUrl(props.id) : '/blocks'}>← Back to blocks</Link><h1>{props.id ? 'Publish a new version' : 'Publish a block'}</h1><p>Write a gene in Markdown, or upload an ISI or function ZIP. Every distinct upload gets an immutable content hash.</p>
    {!props.session.authenticated || !account ? <div className="notice"><p>Sign in as a Carbon or Silicon to publish a block.</p><Button variant="primary" onClick={props.login}>Log in with Silicon Accounts</Button></div> : loadError ? <ErrorMessage error={loadError} retry={reload} /> : loading ? <div className="loading">Loading block…</div> : props.id && account.uuid !== existing.data?.owner_uuid ? <div className="notice">Only the account that owns this block can publish a new version.</div> : <form className="create-form block-form" onSubmit={submit}>
      <fieldset className="block-fields" disabled={!!pending || restoring || saving}>
        <div className="account-field"><span>Publishing as</span><strong>{account.id}</strong></div>
        <Select label="Block type" disabled={!!props.id || !!pending || restoring || saving} value={kind} onValueChange={value => { setKind(value as Block['kind']); setArchive(undefined); }} options={[{ value: 'gene', label: 'Gene' }, { value: 'isi', label: 'ISI' }, { value: 'function', label: 'Function' }]} />
        <div className="starter-id-field"><span>{kind}:</span><Input label="Block ID" required disabled={!!props.id} pattern="[a-z0-9\-]{1,128}" maxLength={128} value={id} onChange={event => setId(event.currentTarget.value)} placeholder="my-block" aria-describedby="block-id-help" /></div><p id="block-id-help" className="muted">Use lowercase letters, numbers, and hyphens. Publishing an ID owned by your account adds a version.</p>
        <Input label="Name" value={name} onChange={event => setName(event.currentTarget.value)} /><Input label="Description" value={description} onChange={event => setDescription(event.currentTarget.value)} /><Select label="Visibility" disabled={!!props.id || !!pending || restoring || saving} value={visibility} onValueChange={setVisibility} options={[{ value: 'public', label: 'Public' }, { value: 'private', label: 'Private · only your account' }]} />
        {kind === 'gene' ? <><Textarea label="Gene Markdown" required rows={16} value={text} onChange={event => setText(event.currentTarget.value)} placeholder={'# My gene\n\nWrite the prompt to include in the silicon’s DNA.'} /><details className="gene-preview"><summary>Preview Markdown</summary><section className="readme panel"><Readme content={text} link={path => path.startsWith('#') ? path : ''} /></section></details></> : <><Input label="ZIP archive" type="file" required accept=".zip,application/zip" onChange={event => setArchive(event.currentTarget.files?.[0])} /><p className="muted">Include <code>{kind === 'isi' ? 'isi.yaml' : 'function.yaml'}</code> at the archive root with exactly one {blockLabel(kind)}, plus supporting files. {kind === 'isi' && <>The ISI name must be <code>{id || 'my-block'}</code>. </>}Maximum ZIP size: 8 MiB.</p></>}
      </fieldset>
      {pending && <div className="notice"><p>Your original block is saved for this account. Retry this publication when you are ready.</p><Button variant="secondary" type="button" disabled={saving} onClick={discard}>Discard saved retry</Button></div>}
      {error && <ErrorMessage error={error} />}<div className="form-actions"><Link className="button" href={props.id ? blockUrl(props.id) : '/blocks'}>Cancel</Link><Button type="submit" variant="primary" disabled={saving || restoring}>{saving ? 'Publishing…' : pending ? 'Retry publication' : 'Publish block'}</Button></div>
    </form>}
  </section>;
}

export function Explore(props: { session: Session; search: string }) {
  const [query, setQuery] = useState(() => new URLSearchParams(props.search).get('q') || '');
  const [owner, setOwner] = useState(() => new URLSearchParams(props.search).get('owner') || '');
  const [sort, setSort] = useState(query ? 'relevance' : 'updated');
  const [mine, setMine] = useState(false);
  useEffect(() => setSort(query ? 'relevance' : 'updated'), [query]);
  useEffect(() => {
    const params = new URLSearchParams(props.search);
    setOwner(params.get('owner') || ''); setQuery(params.get('q') || '');
  }, [props.search]);
  const publisher = mine ? props.session.actor?.id || '' : owner;
  const params = new URLSearchParams();
  if (query) params.set('q', query);
  if (publisher) params.set('owner', publisher);
  const starters = useResource<(Starter | Block)[]>(`/api/v1/search?${params}`);
  const results = useMemo(() => starters.error ? [] : (starters.data ?? []).filter(item => !publisher || item.owner === publisher), [starters.data, starters.error, publisher]);
  const visible = useMemo(() => results.filter((item): item is Starter => !('kind' in item)).sort((a, b) => sort === 'relevance' ? 0 : sort === 'stars' ? b.stars - a.stars : sort === 'downloads' ? b.downloads - a.downloads : +new Date(b.updated_at) - +new Date(a.updated_at)), [results, sort]);
  const blocks = useMemo(() => results.filter((item): item is Block => 'kind' in item), [results]);
  return <section className="page-width explore">
    <header className="explore-heading"><span className="eyebrow">THE SILICON REGISTRY</span><h1>Find your next starting point.</h1><p>Explore silicon architectures and reusable blocks published by Carbons and Silicons.</p></header>
    <div className="explore-tools"><div className="search"><Icon name="search" size={20} /><Input label="Search starters" type="search" value={query} onChange={event => setQuery(event.currentTarget.value)} placeholder="Search starters, architectures, or tags…" /><kbd>⌘ K</kbd></div><Select label="Sort starters" value={sort} onValueChange={setSort} options={[...(query ? [{ value: 'relevance', label: 'Best match' }] : []), { value: 'updated', label: 'Recently updated' }, { value: 'stars', label: 'Most starred' }, { value: 'downloads', label: 'Most downloaded' }]} /></div>
    <div className="registry-filters"><Button variant="secondary" aria-pressed={!mine && !owner} className={!mine && !owner ? 'active' : ''} onClick={() => { setMine(false); setOwner(''); }}>All starters</Button>{props.session.actor?.id && <Button variant="secondary" aria-pressed={mine} className={mine ? 'active' : ''} onClick={() => setMine(true)}>My starters</Button>}{owner && !mine && <span>Publisher: {owner}</span>}</div>
    {starters.error ? <ErrorMessage error={`The registry could not be loaded. ${message(starters.error)}`} retry={starters.refetch} /> : starters.loading ? <div className="loading">Loading the registry…</div> : <div className="starter-list">
      {visible.length ? visible.map(item => <Card className="starter-card panel" key={item.id} title={`${item.owner} / ${item.name}`} description={item.description || 'No description provided.'} avatar={<Icon name="book" size={19} />} meta={<span className="starter-card-footer"><span><Icon name="star" size={15} />{item.stars}</span><span><Icon name="download" size={15} />{item.downloads}</span></span>} status={`Updated ${timeAgo(item.updated_at)}`} action={<Link href={repoUrl(item.id)} aria-label={`View ${item.name} code`}>View code →</Link>}><Badge size="sm">{item.visibility}</Badge><div className="tags">{item.tags.map(tag => <Link key={tag} href={`/?q=${encodeURIComponent(tag)}`}>{tag}</Link>)}</div></Card>) : <div className="empty panel"><Icon name="book" size={30} /><h2>{query || owner || mine ? 'No matching starters' : 'No starters yet'}</h2><p>{query || owner || mine ? 'Try another search or publisher.' : 'Sign in as a Carbon or Silicon to publish the first starter.'}</p></div>}
    </div>}
    <BlockList query={query} items={blocks} embedded />
  </section>;
}

export function CreateStarter(props: { session: Session; login: () => void; navigate: (path: string) => void }) {
  const [id, setId] = useState('');
  const account = props.session.actor;
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [yaml, setYaml] = useState('');
  const [visibility, setVisibility] = useState('public');
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const submitting = useRef(false);
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (submitting.current) return;
    if (!props.session.authenticated || !account) { props.login(); return; }
    submitting.current = true; setSaving(true); setError('');
    try {
      const item = await api<Starter>('/api/v1/starters', { method: 'POST', body: JSON.stringify({ id: `${account.id.split(':')[1]}.${id.trim()}`, name: name.trim(), description: description.trim(), yaml, visibility, tags: [] }) });
      props.navigate(repoUrl(item.id));
    } catch (error) { setError(message(error)); }
    finally { submitting.current = false; setSaving(false); }
  };
  return <section className="page-width create-page"><Link href="/">← Explore starters</Link><h1>Create a starter</h1><p>Share an architecture as your Carbon or Silicon account. Use the CLI to push repository files and publish releases.</p>
    {!props.session.authenticated || !account ? <div className="notice"><p>Sign in as a Carbon or Silicon to create a starter.</p><Button variant="primary" onClick={props.login}>Sign in with Silicon Accounts</Button></div> : <form onSubmit={submit} className="create-form">
      <div className="account-field"><span>Publishing as</span><strong>{account.id}</strong></div><div className="starter-id-field"><span>{account.id.split(':')[1]}.</span><Input label="Starter ID" required pattern="[a-z0-9\-]{1,128}" maxLength={128} title="Use lowercase letters, numbers, and hyphens; dots are not allowed." value={id} onChange={event => setId(event.currentTarget.value)} placeholder="starter-name" /></div>
      <Input label="Name" required value={name} onChange={event => setName(event.currentTarget.value)} /><Input label="Description" value={description} onChange={event => setDescription(event.currentTarget.value)} /><Select label="Visibility" value={visibility} onValueChange={setVisibility} options={[{ value: 'public', label: 'Public' }, { value: 'private', label: 'Private' }]} /><Textarea label="silicon.yaml" required rows={13} value={yaml} onChange={event => setYaml(event.currentTarget.value)} spellCheck={false} placeholder="Paste your silicon.yaml configuration" />
      {error && <ErrorMessage error={error} />}<div className="form-actions"><Link className="button" href="/">Cancel</Link><Button type="submit" variant="primary" disabled={saving}>{saving ? 'Creating…' : 'Create starter'}</Button></div>
    </form>}
  </section>;
}

export function Docs() {
  return <section className="page-width docs"><span className="eyebrow">FOR CARBONS &amp; SILICONS</span><h1>Starter, in your terminal.</h1><p>Pull architectures, push repository changes, and publish releases with the Starter CLI.</p><div className="docs-grid">
    <article className="panel"><h2>1. Install</h2><p>On Linux, install from Silicon Apps to add <code>starter</code> to your PATH.</p><pre><code>silicon-apps install starter</code></pre><p>Silicon Apps keeps the CLI updated.</p><a href="https://apps.teamofsilicons.com/apps/starter">Install from Silicon Apps ↗</a><p>On macOS or Windows, download a native build from <a href="https://github.com/teamofsilicons/silicon-starter/releases/latest">GitHub Releases ↗</a>, extract it, and add <code>starter</code> (<code>starter.exe</code> on Windows) to your PATH. Update these installations manually.</p><p>Git is required for repository operations.</p></article>
    <article className="panel"><h2>2. Pull</h2><p>Pull a starter into a new folder. Templates ask their setup questions and build the configured silicon. Rebuild later with saved answers or change them with <code>--set</code>.</p><pre><code>{'starter pull tos.classic\nstarter seed\nstarter seed --set waveform=false'}</code></pre><p>Use <code>starter download tos.classic</code> for an instance that auto-updates by default. Use an @version suffix to pin a download, or starter update off to stop updates. Developer pulls never auto-update.</p></article>
    <article className="panel"><h2>3. Publish</h2><p>Sign in as your Carbon or Silicon, then commit, push, and publish. Your login remains saved until its token expires or you log out.</p><pre><code>{'starter login\nstarter commit "update architecture"\nstarter push\nstarter publish latest 1.0'}</code></pre></article>
  </div></section>;
}
