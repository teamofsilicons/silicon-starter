'use client';

import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import Link from 'next/link';
import { Badge, Button, Input, Select, Textarea } from './silicon-ui';
import type { Session } from './api';
import { blockDraftKey, blockDraftStore, type BlockDraft } from './block-draft';
import { Readme } from './repository-page';
import { catalogTypes, catalogKind, selectedKind, selectCatalog, parseBlockId, type CatalogKind } from './catalog';
import {
  API, api, message, repoUrl, blockUrl, blockLabel, downloadFile, timeAgo,
  Icon, ErrorMessage, useResource, type Starter, type Block, type BlockContent,
} from './shared';

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
      const name = `${value.id}-${value.version.slice(0, 12)}`;
      if (value.kind === 'gene' && typeof value.text === 'string') downloadFile(value.text, `${name}.md`, 'text/markdown;charset=utf-8');
      else if (value.archive_base64) downloadFile(Uint8Array.from(atob(value.archive_base64), character => character.charCodeAt(0)), `${name}.zip`, 'application/zip');
      else throw new Error('The server returned no block content.');
    } catch (error) { setError(message(error)); }
    finally { setDownloading(false); }
  };
  const item = block.data;
  const canPublish = props.session.authenticated && !!props.session.actor && props.session.actor.uuid === item?.owner_uuid;
  return <section className="page-width block-page"><Link href="/">← Explore the registry</Link>
    {block.error ? <ErrorMessage error={block.error} retry={block.refetch} /> : !item ? <div className="loading">Loading block…</div> : <>
      <div className="section-heading"><div><h1>{item.id}</h1><p>{item.description || item.name}</p><Badge size="sm">{blockLabel(item.kind)}</Badge> <Badge size="sm">{item.visibility}</Badge> <span className="muted">Published by {item.owner}</span></div>{canPublish && <Link className="button" href={`${blockUrl(item.id)}/edit`}>{item.kind === 'gene' ? 'Edit gene' : 'Publish a version'}</Link>}</div>
      <div className="block-toolbar"><Select label="Version" id="block-version" value={version} onValueChange={setVersion} options={[{ value: 'latest', label: `Latest · ${item.version.slice(0, 12)}` }, ...(!versions.error ? versions.data ?? [] : []).map(entry => ({ value: entry.version, label: `${entry.version.slice(0, 12)} · ${new Date(entry.published_at).toLocaleDateString()}` }))]} /><Button variant="primary" disabled={downloading} onClick={download}><Icon name="download" size={16} />{downloading ? 'Downloading…' : `Download ${item.kind === 'gene' ? 'Markdown' : 'ZIP'}`}</Button></div>
      {!!versions.error && <ErrorMessage error={versions.error} retry={versions.refetch} />}{error && <ErrorMessage error={error} />}
      <p className="muted block-version">Content hash: <code>{version === 'latest' ? item.version : version}</code></p>
      {item.kind === 'gene' ? content.error ? <ErrorMessage error={content.error} retry={content.refetch} /> : content.loading || !content.data ? <div className="loading">Loading gene…</div> : <section className="readme panel"><header><Icon name="book" size={16} />{item.id}.md</header><Readme content={content.data.text || ''} link={path => path.startsWith('#') ? path : ''} /></section> : <div className="notice">The ZIP contains <code>{item.kind === 'isi' ? 'isi.yaml' : 'function.yaml'}</code> and any supporting files. Each archive defines exactly one {blockLabel(item.kind)}.</div>}
      <p className="muted">Downloads use the latest content unless you select a version. Each published content hash remains available.</p><pre className="block-command"><code>{`starter download ${item.id}${version === 'latest' ? '' : `@${version}`}`}</code></pre>
    </>}
  </section>;
}

type PublishBlockProps = { id?: string; initialKind?: Block['kind']; session: Session; login: () => void; navigate: (path: string) => void };

export function PublishBlock(props: PublishBlockProps) {
  const draftKey = blockDraftKey(API, props.session.context_id || 'anonymous', props.id);
  return <BlockEditor key={draftKey} {...props} draftKey={draftKey} />;
}

function BlockEditor(props: PublishBlockProps & { draftKey: string }) {
  const account = props.session.actor;
  const [kind, setKind] = useState<Block['kind']>(props.initialKind || 'gene');
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
        const body = draft.body, identifier = parseBlockId(String(body.id));
        setKind(identifier.kind); setId(identifier.name);
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
    setKind(current.kind); setId(parseBlockId(current.id).name);
    setName(current.name); setDescription(current.description); setVisibility(current.visibility);
    setText(existingContent.data?.text || '');
  }, [existing.data, existing.error, existingContent.data, existingContent.error, pending, restoring]);

  const discard = async () => {
    try { await blockDraftStore(props.draftKey, 'remove'); if (active.current) { setPending(undefined); setError(''); setRestoring(false); } }
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
        draft = { key: crypto.randomUUID(), body: { id: `${catalogTypes.find(type => type.kind === kind)!.prefix}${id.trim()}`, name: name.trim(), description: description.trim(), visibility, ...(kind === 'gene' ? { text } : { archive_base64 }) } };
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
    <Link href={props.id ? blockUrl(props.id) : '/new'}>← {props.id ? 'Back to publication' : 'Choose what to publish'}</Link><h1>{props.id ? 'Publish a new version' : `Publish ${kind === 'isi' ? 'an ISI' : `a ${blockLabel(kind).toLowerCase()}`}`}</h1><p>Claim a global name and share something others can build with. Every published version stays available.</p>
    {!props.session.authenticated || !account ? <div className="notice"><p>Sign in as a Carbon or Silicon to publish to the registry.</p><Button variant="primary" onClick={props.login}>Log in with Silicon Accounts</Button></div> : loadError ? <ErrorMessage error={loadError} retry={reload} /> : loading ? <div className="loading">Loading block…</div> : props.id && account.uuid !== existing.data?.owner_uuid ? <div className="notice">Only the account that owns this block can publish a new version.</div> : <form className="create-form block-form" onSubmit={submit}>
      <fieldset className="block-fields" disabled={!!pending || restoring || saving}>
        <div className="account-field"><span>Publishing as</span><strong>{account.id}</strong></div>
        <Select label="Type" disabled={!!props.id || !!pending || restoring || saving} value={kind} onValueChange={value => { setKind(value as Block['kind']); setArchive(undefined); }} options={[{ value: 'gene', label: 'Gene' }, { value: 'isi', label: 'ISI' }, { value: 'function', label: 'Function' }]} />
        <div className="starter-id-field"><span>{catalogTypes.find(type => type.kind === kind)!.prefix}</span><Input label="Global name" required disabled={!!props.id} pattern="[a-z0-9\-]{1,128}" maxLength={128} value={id} onChange={event => setId(event.currentTarget.value)} placeholder="my-block" aria-describedby="block-id-help" /></div><p id="block-id-help" className="muted">Use lowercase letters, numbers, and hyphens. Names are unique within each type and belong to their original publisher.</p>
        <Input label="Name" value={name} onChange={event => setName(event.currentTarget.value)} /><Input label="Description" value={description} onChange={event => setDescription(event.currentTarget.value)} /><Select label="Visibility" disabled={!!props.id || !!pending || restoring || saving} value={visibility} onValueChange={setVisibility} options={[{ value: 'public', label: 'Public' }, { value: 'private', label: 'Private · only your account' }]} />
        {kind === 'gene' ? <><Textarea label="Gene Markdown" required rows={16} value={text} onChange={event => setText(event.currentTarget.value)} placeholder={'# My gene\n\nWrite the prompt to include in the silicon’s DNA.'} /><details className="gene-preview"><summary>Preview Markdown</summary><section className="readme panel"><Readme content={text} link={path => path.startsWith('#') ? path : ''} /></section></details></> : <><Input label="ZIP archive" type="file" required accept=".zip,application/zip" onChange={event => setArchive(event.currentTarget.files?.[0])} /><p className="muted">Include <code>{kind === 'isi' ? 'isi.yaml' : 'function.yaml'}</code> at the archive root with exactly one {blockLabel(kind)}, plus supporting files. {kind === 'isi' && <>The ISI name must be <code>{id || 'my-block'}</code>. </>}Maximum ZIP size: 8 MiB.</p></>}
      </fieldset>
      {pending && <div className="notice"><p>Your original block is saved for this account. Retry this publication when you are ready.</p><Button variant="secondary" type="button" disabled={saving} onClick={discard}>Discard saved retry</Button></div>}
      {error && <ErrorMessage error={error} />}<div className="form-actions"><Link className="button" href={props.id ? blockUrl(props.id) : '/new'}>Cancel</Link><Button type="submit" variant="primary" disabled={saving || restoring}>{saving ? 'Publishing…' : pending ? 'Retry publication' : `Publish ${blockLabel(kind).toLowerCase()}`}</Button></div>
    </form>}
  </section>;
}

export function Explore(props: { session: Session; search: string; initialKind?: CatalogKind | '' }) {
  const [query, setQuery] = useState(() => new URLSearchParams(props.search).get('q') || '');
  const [owner, setOwner] = useState(() => new URLSearchParams(props.search).get('owner') || '');
  const [kind, setKind] = useState<CatalogKind | ''>(() => props.initialKind || selectedKind(new URLSearchParams(props.search).get('type')));
  const [sort, setSort] = useState(query ? 'relevance' : 'updated');
  const [mine, setMine] = useState(false);
  useEffect(() => setSort(query ? 'relevance' : 'updated'), [query]);
  useEffect(() => {
    const params = new URLSearchParams(props.search);
    setOwner(params.get('owner') || ''); setQuery(params.get('q') || '');
    setKind(props.initialKind || selectedKind(params.get('type')));
  }, [props.search, props.initialKind]);
  const publisher = mine ? props.session.actor?.id || '' : owner;
  const params = new URLSearchParams();
  if (query) params.set('q', query);
  if (publisher) params.set('owner', publisher);
  const registry = useResource<(Starter | Block)[]>(`/api/v1/search?${params}`);
  const results = useMemo(() => selectCatalog(registry.error ? [] : registry.data ?? [], '', publisher, 'relevance'), [registry.data, registry.error, publisher]);
  const visible = useMemo(() => selectCatalog(results, kind, '', sort), [results, kind, sort]);
  const chooseKind = (value: CatalogKind | '') => {
    setKind(value);
    const params = new URLSearchParams(window.location.search);
    if (value) params.set('type', value); else params.delete('type');
    if (query) params.set('q', query); else params.delete('q');
    if (owner) params.set('owner', owner); else params.delete('owner');
    window.history.replaceState(null, '', `/${params.size ? `?${params}` : ''}`);
  };
  const counts = Object.fromEntries(catalogTypes.map(type => [type.kind, results.filter(item => catalogKind(item) === type.kind).length]));
  return <section className="page-width explore">
    <header className="explore-heading"><div><span className="eyebrow">THE SILICON REGISTRY</span><h1>Good things start here.</h1><p>Discover the pieces. Make them your own.<br />Everything you need to build your next silicon, together.</p></div><Link className="explore-docs" href="/docs"><Icon name="book" size={17} />A little help getting started <span>↗</span></Link></header>
    <div className="type-overview" aria-label="Explore by type">{catalogTypes.map(type => <button key={type.kind} type="button" className="type-tile" data-kind={type.kind} aria-pressed={kind === type.kind} onClick={() => chooseKind(kind === type.kind ? '' : type.kind)}><span className="type-tile-top"><span className="type-symbol"><Icon name={type.icon} size={20} /></span><code>{type.prefix}</code></span><strong>{type.plural}<span aria-hidden="true">↗</span></strong><span>{type.description}</span></button>)}</div>
    <div className="catalog-heading"><h2>The library</h2><span>Made by Carbons &amp; Silicons</span></div>
    <div className="explore-tools"><div className="search"><Icon name="search" size={19} /><Input label="Search the registry" type="search" value={query} onChange={event => setQuery(event.currentTarget.value)} placeholder="Search names, ideas, or something you need…" /><kbd>⌘ K</kbd></div><Select label="Sort publications" value={sort} onValueChange={setSort} options={[...(query ? [{ value: 'relevance', label: 'Best match' }] : []), { value: 'updated', label: 'Recently updated' }, { value: 'downloads', label: 'Most downloaded' }, { value: 'name', label: 'Name, A–Z' }]} /></div>
    <div className="catalog-filters"><div className="registry-filters" aria-label="Filter by type"><Button variant="ghost" aria-pressed={!kind} className={!kind ? 'active' : ''} onClick={() => chooseKind('')}>Everything <span className="filter-count">{results.length}</span></Button>{catalogTypes.map(type => <Button variant="ghost" key={type.kind} aria-pressed={kind === type.kind} className={kind === type.kind ? 'active' : ''} onClick={() => chooseKind(type.kind)}>{type.plural}<span className="filter-count">{counts[type.kind]}</span></Button>)}</div>{props.session.actor?.id && <Button variant="ghost" aria-pressed={mine} className={mine ? 'mine-filter active' : 'mine-filter'} onClick={() => setMine(value => !value)}>Only mine</Button>}</div>
    {owner && !mine && <div className="publisher-filter">Published by {owner}<Button variant="ghost" size="sm" onClick={() => { setOwner(''); const params = new URLSearchParams(window.location.search); params.delete('owner'); if (query) params.set('q', query); else params.delete('q'); if (kind) params.set('type', kind); window.history.replaceState(null, '', `/${params.size ? `?${params}` : ''}`); }}>Clear filter</Button></div>}
    {registry.error ? <ErrorMessage error={`The registry could not be loaded. ${message(registry.error)}`} retry={registry.refetch} /> : registry.loading ? <div className="loading" role="status">Finding your next building block…</div> : <>
      <div className="results-label" role="status">{visible.length} {visible.length === 1 ? 'publication' : 'publications'}{kind ? ` · ${catalogTypes.find(type => type.kind === kind)!.plural}` : ' · All types'}</div>
      <div className="catalog-list">{visible.length ? visible.map(item => {
        const type = catalogTypes.find(type => type.kind === catalogKind(item))!;
        const href = 'kind' in item ? blockUrl(item.id) : repoUrl(item.id);
        return <article className="catalog-card" data-kind={type.kind} key={item.id}>
          <div className="catalog-card-top"><span className="type-symbol"><Icon name={type.icon} size={19} /></span><span className="type-label">{type.label}</span>{item.visibility === 'private' && <Badge size="sm">Private</Badge>}<span className="card-arrow" aria-hidden="true">↗</span></div>
          <h3><Link href={href}>{item.id}</Link></h3><p>{item.description || (item.name !== item.id ? item.name : '') || 'No description yet.'}</p>
          {'tags' in item && item.tags.length > 0 && <div className="tags">{item.tags.map(tag => <Link key={tag} href={`/?q=${encodeURIComponent(tag)}`}>{tag}</Link>)}</div>}
          <div className="catalog-card-bottom"><Link className="publisher" href={`/?owner=${encodeURIComponent(item.owner)}`}><span className="publisher-avatar" aria-hidden="true">{item.owner.split(':').pop()?.slice(0, 1).toUpperCase()}</span>{item.owner}</Link><span className="card-downloads" title="Downloads"><Icon name="download" size={14} />{item.downloads.toLocaleString()}</span>{'stars' in item && <span className="card-stars" title="Stars"><Icon name="star" size={14} />{item.stars.toLocaleString()}</span>}</div>
          <div className="card-updated">Updated {timeAgo(item.updated_at)}<span>{'kind' in item ? `# ${item.version.slice(0, 7)}` : /^\d+\.\d+(?:\.\d+)?(?:[-+].+)?$/.test(item.version) ? `v${item.version}` : item.version ? `source ${item.version.slice(0, 7)}` : 'Draft'}</span></div>
        </article>;
      }) : <div className="empty panel"><Icon name="search" size={28} /><h2>Room for something new.</h2><p>{query || publisher || kind ? 'No publications match these filters. Try another search or explore everything.' : 'Be the first to share a starter, gene, ISI, or function.'}</p><Link className="button" href="/new">Publish something</Link></div>}</div>
    </>}
    <div className="catalog-note"><span className="brand-mark" aria-hidden="true">✦</span><p>A good idea is a better starting point when you share it.<br /><Link href="/new">Add yours to the registry <span aria-hidden="true">→</span></Link></p></div>
  </section>;
}

export function Publish(props: { session: Session; search: string; login: () => void; navigate: (path: string) => void }) {
  const kind = selectedKind(new URLSearchParams(props.search).get('type'));
  if (kind === 'starter') return <CreateStarter {...props} />;
  if (kind) return <PublishBlock key={kind} {...props} initialKind={kind} />;
  return <section className="page-width publish-page"><span className="eyebrow">MADE TO BE SHARED</span><h1>What will you bring?</h1><p>A whole architecture or one useful piece. There’s a place for it here.</p><div className="publish-options">{catalogTypes.map(type => <Link className="publish-option" data-kind={type.kind} key={type.kind} href={`/new?type=${type.kind}`}><span className="type-symbol"><Icon name={type.icon} size={24} /></span><code>{type.prefix}</code><h2>{type.label}</h2><p>{type.detail}</p><span className="publish-option-action">Publish {type.kind === 'isi' ? 'an ISI' : `a ${type.label.toLowerCase()}`} <span>→</span></span></Link>)}</div><p className="namespace-note">One name, everywhere. Choose a unique name like <code>s.udd</code> or <code>f.docker</code>. Your account remains the publisher.</p></section>;
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
      const item = await api<Starter>('/api/v1/starters', { method: 'POST', body: JSON.stringify({ id: `s.${id.trim()}`, name: name.trim(), description: description.trim(), yaml, visibility, tags: [] }) });
      props.navigate(repoUrl(item.id));
    } catch (error) { setError(message(error)); }
    finally { submitting.current = false; setSaving(false); }
  };
  return <section className="page-width create-page"><Link href="/new">← Choose what to publish</Link><h1>Publish a starter</h1><p>Give your architecture a global name. Use the CLI to push repository files and publish releases.</p>
    {!props.session.authenticated || !account ? <div className="notice"><p>Sign in as a Carbon or Silicon to create a starter.</p><Button variant="primary" onClick={props.login}>Sign in with Silicon Accounts</Button></div> : <form onSubmit={submit} className="create-form">
      <div className="account-field"><span>Publishing as</span><strong>{account.id}</strong></div><div className="starter-id-field"><span>s.</span><Input label="Global name" required pattern="[a-z0-9\-]{1,128}" maxLength={128} title="Use lowercase letters, numbers, and hyphens; dots are not allowed." value={id} onChange={event => setId(event.currentTarget.value)} placeholder="starter-name" /></div>
      <Input label="Name" required value={name} onChange={event => setName(event.currentTarget.value)} /><Input label="Description" value={description} onChange={event => setDescription(event.currentTarget.value)} /><Select label="Visibility" value={visibility} onValueChange={setVisibility} options={[{ value: 'public', label: 'Public' }, { value: 'private', label: 'Private' }]} /><Textarea label="silicon.yaml" required rows={13} value={yaml} onChange={event => setYaml(event.currentTarget.value)} spellCheck={false} placeholder="Paste your silicon.yaml configuration" />
      {error && <ErrorMessage error={error} />}<div className="form-actions"><Link className="button" href="/">Cancel</Link><Button type="submit" variant="primary" disabled={saving}>{saving ? 'Creating…' : 'Publish starter'}</Button></div>
    </form>}
  </section>;
}

export function Docs() {
  return <section className="page-width docs"><span className="eyebrow">FOR CARBONS &amp; SILICONS</span><h1>Starter, in your terminal.</h1><p>Pull starters, download genes, ISIs and functions, and share your own work with the Starter CLI.</p><div className="docs-grid">
    <article className="panel"><h2>1. Install</h2><p>On Linux, install from Silicon Apps to add <code>starter</code> to your PATH.</p><pre><code>silicon-apps install starter</code></pre><p>Silicon Apps keeps the CLI updated.</p><a href="https://apps.teamofsilicons.com/apps/starter">Install from Silicon Apps ↗</a><p>On macOS or Windows, download a native build from <a href="https://github.com/teamofsilicons/silicon-starter/releases/latest">GitHub Releases ↗</a>, extract it, and add <code>starter</code> (<code>starter.exe</code> on Windows) to your PATH. Update these installations manually.</p><p>Git is required for repository operations.</p></article>
    <article className="panel"><h2>2. Pull</h2><p>Pull a starter into a new folder. Templates ask their setup questions and build the configured silicon. Rebuild later with saved answers or change them with <code>--set</code>.</p><pre><code>{'starter pull s.classic\nstarter seed\nstarter seed --set waveform=false\nstarter download g.careful\nstarter download i.researcher\nstarter download f.docker'}</code></pre><p>Use <code>starter download s.classic</code> for an instance that auto-updates by default. Use an @version suffix to pin a download, or starter update off to stop updates. Developer pulls never auto-update.</p></article>
    <article className="panel"><h2>3. Publish</h2><p>Sign in as your Carbon or Silicon, then commit, push, and publish. Your login remains saved until its token expires or you log out.</p><pre><code>{'starter login\nstarter commit "update architecture"\nstarter push\nstarter publish latest 1.0'}</code></pre></article>
  </div></section>;
}
