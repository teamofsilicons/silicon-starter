import assert from 'node:assert/strict';
import { blockDraftKey } from '../src/block-draft.ts';
import { catalogKind, parseBlockId, selectCatalog, selectedKind } from '../src/catalog.ts';

const common = { name: '', description: '', owner: 'c:alice', owner_uuid: 'alice', visibility: 'public' as const, version: '1', downloads: 0, updated_at: '2026-01-01' };
const starter = { ...common, id: 's.demo', stars: 0, tags: [], yaml: '' };
const gene = { ...common, id: 'g.demo', kind: 'gene' as const, downloads: 4 };
const isi = { ...common, id: 'i.demo', kind: 'isi' as const, owner: 'si:bob', updated_at: '2026-02-01' };
const fn = { ...common, id: 'f.demo', kind: 'function' as const, downloads: 8 };
const items = [starter, gene, isi, fn];
assert.deepEqual(selectCatalog(items, '', '', 'downloads').map(item => item.id), ['f.demo', 'g.demo', 's.demo', 'i.demo']);
assert.deepEqual(selectCatalog(items, 'isi', 'si:bob', 'updated'), [isi]);
assert.deepEqual(selectCatalog(items, 'isi', 'c:alice', 'updated'), []);
assert.deepEqual(selectCatalog(items, '', '', 'relevance'), items);
assert.equal(catalogKind(starter), 'starter');
assert.equal(selectedKind('function'), 'function');
assert.equal(selectedKind('unknown'), '');
for (const [kind, prefix] of [['gene', 'g.'], ['isi', 'i.'], ['function', 'f.']] as const) {
  assert.deepEqual(parseBlockId(`${prefix}demo`), { kind, name: 'demo' });
  assert.deepEqual(parseBlockId(`${kind}:demo`), { kind, name: 'demo' });
  assert.equal(blockDraftKey('api', 'account-a', `${prefix}demo`), blockDraftKey('api', 'account-a', `${kind}:demo`));
}
assert.notEqual(blockDraftKey('api', 'account-a', 'g.demo'), blockDraftKey('api', 'account-b', 'g.demo'));
assert.equal(blockDraftKey('api', 'account-a'), JSON.stringify(['api', 'account-a', 'new']));
assert.throws(() => parseBlockId('broken'), /original content has been kept/);
console.log('Catalog filtering, ordering, and legacy draft IDs passed.');
