// IndexedDB preserves large ZIP drafts without sending them to a different login.
export type BlockDraft = { body: Record<string, unknown>; key: string };
export function blockDraftKey(api: string, context: string, id?: string) {
  return JSON.stringify([api, context, id || 'new']);
}
export async function blockDraftStore(key: string, action: 'read' | 'write' | 'remove', draft?: BlockDraft): Promise<BlockDraft | undefined> {
  const database = await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open('starter-block-drafts', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('drafts');
    request.onerror = () => reject(new Error('Could not open private block drafts. Keep this page open and retry.'));
    request.onsuccess = () => resolve(request.result);
  });
  try {
    return await new Promise<BlockDraft | undefined>((resolve, reject) => {
      const transaction = database.transaction('drafts', action === 'read' ? 'readonly' : 'readwrite');
      const store = transaction.objectStore('drafts');
      const request = action === 'read' ? store.get(key) : action === 'remove' ? store.delete(key) : store.put(draft, key);
      transaction.oncomplete = () => resolve(action === 'read' ? request.result : draft);
      transaction.onerror = transaction.onabort = () => reject(new Error('Could not save the original block draft. No publication was sent.'));
    });
  } finally { database.close(); }
}
