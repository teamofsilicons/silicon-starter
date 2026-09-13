import { render } from 'solid-js/web';
import { For, Show, createMemo, createResource, createSignal } from 'solid-js';
import './styles.css';

type Starter = { id: string; name: string; description: string; org: string; version: string; downloads: number; stars: number; updated: string; tags: string[]; accent: string };
const sample: Starter[] = [
  { id: 'tos.agents', name: 'Agents', description: 'Composable building blocks for reliable silicon agents.', org: 'teamofsilicons', version: '2.4', downloads: 1240, stars: 86, updated: '2h ago', tags: ['agents', 'core'], accent: '#8b5cf6' },
  { id: 'tos.vision', name: 'Vision Lab', description: 'A production-ready vision pipeline with memory and tools.', org: 'teamofsilicons', version: '1.8', downloads: 842, stars: 54, updated: 'yesterday', tags: ['vision', 'multimodal'], accent: '#06b6d4' },
  { id: 'tos.knowledge', name: 'Knowledge Graph', description: 'Turn your source material into a queryable silicon memory.', org: 'teamofsilicons', version: '0.9', downloads: 611, stars: 39, updated: '3d ago', tags: ['memory', 'search'], accent: '#f59e0b' },
  { id: 'lab.orbit', name: 'Orbit', description: 'A minimal coordinator for multi-silicon workflows.', org: 'lab', version: '1.2', downloads: 398, stars: 22, updated: '5d ago', tags: ['orchestration'], accent: '#22c55e' },
];

async function loadStarters(): Promise<Starter[]> {
  const base = import.meta.env.VITE_API_URL || 'https://backend.starter.teamofsilicons.com';
  for (const path of ['/api/v1/starters', '/api/starters']) {
    try {
      const response = await fetch(`${base}${path}`);
      if (!response.ok) continue;
      const data = await response.json();
      const items = Array.isArray(data) ? data : data.items || data.starters || [];
      if (items.length) return items.map((item: Partial<Starter>, i: number) => ({ ...sample[i % sample.length], ...item, accent: item.accent || sample[i % sample.length].accent }));
    } catch { /* offline shell uses the local preview data */ }
  }
  return sample;
}

const Icon = (props: { name: string; size?: number }) => {
  const size = props.size || 18;
  const paths: Record<string, string> = { search: 'M11 19a8 8 0 1 1 0-16 8 8 0 0 1 0 16Zm5.65-2.35L21 21', plus: 'M12 5v14M5 12h14', arrow: 'M5 12h14m-6-6 6 6-6 6', star: 'm12 3 2.8 5.67 6.2.9-4.5 4.4 1.06 6.2L12 18.25 6.44 21.17 7.5 14.97 3 10.57l6.2-.9L12 3Z', download: 'M12 3v12m0 0 5-5m-5 5-5-5M4 21h16', grid: 'M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z', clock: 'M12 6v6l4 2', menu: 'M4 6h16M4 12h16M4 18h16' };
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[props.name] || paths.grid} /></svg>;
};

function App() {
  const [starters] = createResource(loadStarters);
  const [query, setQuery] = createSignal('');
  const [filter, setFilter] = createSignal('Popular');
  const visible = createMemo(() => (starters() || []).filter((item) => `${item.name} ${item.description} ${item.tags.join(' ')}`.toLowerCase().includes(query().toLowerCase())));
  return <div class="app-shell">
    <aside class="sidebar">
      <div class="brand"><div class="brand-mark">✦</div><span>starter</span></div>
      <div class="side-label">Your workspaces</div>
      <button class="workspace active"><span class="org-dot purple">T</span><span>teamofsilicons</span><span class="chevron">⌄</span></button>
      <button class="workspace"><span class="org-dot cyan">L</span><span>lab</span></button>
      <button class="add-org"><Icon name="plus" size={15} /> Attach organization</button>
      <nav class="nav"><a class="nav-item active"><Icon name="grid" /> Explore</a><a class="nav-item"><Icon name="download" /> My starters</a><a class="nav-item"><Icon name="star" /> Starred</a></nav>
      <div class="sidebar-bottom"><div class="user"><div class="avatar">S</div><div><strong>Shubham</strong><small>Carbon account</small></div><span class="more">•••</span></div></div>
    </aside>
    <main class="main">
      <header class="topbar"><button class="mobile-menu"><Icon name="menu" /></button><div class="crumb"><span>teamofsilicons</span><b>/</b><strong>Explore</strong></div><div class="top-actions"><button class="icon-button" aria-label="Notifications">◌</button><button class="login" onClick={() => { window.location.href = `${import.meta.env.VITE_API_URL || 'https://backend.starter.teamofsilicons.com'}/auth/login`; }}>Log in</button><button class="create"><Icon name="plus" size={16} /> New starter</button></div></header>
      <section class="hero"><div class="eyebrow">THE SILICON REGISTRY</div><h1>Find your next<br /><em>starting point.</em></h1><p>Pre-built silicon architectures, ready to pull, explore, and make yours.</p><div class="search-wrap"><Icon name="search" size={20} /><input aria-label="Search starters" value={query()} onInput={(event) => setQuery(event.currentTarget.value)} placeholder="Search starters, architectures, or tags..." /><kbd>⌘ K</kbd></div><div class="hero-stats"><span><b>2,481</b> starters</span><i></i><span><b>18.2k</b> versions</span><i></i><span><b>94k</b> downloads</span></div></section>
      <section class="content"><div class="section-head"><div><h2>Explore starters</h2><p>Curated architectures from the community</p></div><div class="filters"><For each={['Popular', 'Recently updated', 'Most starred']}>{(name) => <button class={filter() === name ? 'selected' : ''} onClick={() => setFilter(name)}>{name}</button>}</For></div></div><Show when={!starters.loading} fallback={<div class="loading">Loading the registry…</div>}><div class="cards"><For each={visible()} fallback={<div class="empty">No starters match “{query()}”.</div>}>{(item) => <article class="card"><div class="card-top"><div class="starter-icon" style={{ 'background': `${item.accent}1a`, color: item.accent }}>{item.name.slice(0, 1)}</div><button class="star-btn" aria-label={`Star ${item.name}`}><Icon name="star" size={17} /></button></div><div class="card-org">{item.org} <span>·</span> {item.id}</div><h3>{item.name}</h3><p>{item.description}</p><div class="tags"><For each={item.tags}>{(tag) => <span>{tag}</span>}</For></div><div class="card-meta"><span><Icon name="star" size={14} /> {item.stars}</span><span><Icon name="download" size={14} /> {item.downloads.toLocaleString()}</span><span class="updated"><Icon name="clock" size={14} /> {item.updated}</span></div><div class="card-footer"><span>v{item.version}</span><button>View starter <Icon name="arrow" size={15} /></button></div></article>}</For></div></Show></section>
      <footer>Silicon Starter <span>·</span> Open source registry for the silicon community <span class="footer-links">Docs&nbsp;&nbsp; CLI&nbsp;&nbsp; GitHub</span></footer>
    </main>
  </div>;
}
render(() => <App />, document.getElementById('root')!);
