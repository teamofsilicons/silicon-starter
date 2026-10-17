import type { Block, Starter } from './shared';

export const catalogTypes = [
  { kind: 'starter', prefix: 's.', label: 'Starter', plural: 'Starters', icon: 'architecture', description: 'A complete starting point.', detail: 'Silicon architectures, source files, and templates.' },
  { kind: 'gene', prefix: 'g.', label: 'Gene', plural: 'Genes', icon: 'gene', description: 'Shape how a silicon thinks.', detail: 'Reusable prompts and instructions, in Markdown.' },
  { kind: 'isi', prefix: 'i.', label: 'ISI', plural: 'ISIs', icon: 'isi', description: 'Bring another silicon along.', detail: 'Internal silicons with their own configuration.' },
  { kind: 'function', prefix: 'f.', label: 'Function', plural: 'Functions', icon: 'code', description: 'Give your silicon a new skill.', detail: 'Tools and actions, ready to plug in.' },
] as const;
export type CatalogKind = typeof catalogTypes[number]['kind'];
export const catalogKind = (item: Starter | Block): CatalogKind => 'kind' in item ? item.kind : 'starter';
export const selectedKind = (value: string | null): CatalogKind | '' => catalogTypes.find(type => type.kind === value)?.kind || '';
export function selectCatalog(items: (Starter | Block)[], kind: string, owner: string, sort: string) {
  return items.filter(item => (!kind || catalogKind(item) === kind) && (!owner || item.owner === owner))
    .sort((a, b) => sort === 'relevance' ? 0 : sort === 'downloads' ? b.downloads - a.downloads : sort === 'name' ? a.id.localeCompare(b.id) : +new Date(b.updated_at) - +new Date(a.updated_at));
}

export function parseBlockId(id: string): { kind: Block['kind']; name: string } {
  const type = catalogTypes.find(type => type.kind !== 'starter' && (id.startsWith(type.prefix) || id.startsWith(`${type.kind}:`)));
  if (!type || type.kind === 'starter') throw new Error('This saved publication has an invalid identifier. Its original content has been kept.');
  return { kind: type.kind, name: id.slice(id.startsWith(type.prefix) ? type.prefix.length : type.kind.length + 1) };
}
