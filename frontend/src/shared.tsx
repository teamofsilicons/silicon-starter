"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Bot, Dna } from 'lucide-react';
import { createApi } from "./api";
import { Button } from "./silicon-ui";

export type Starter = { id: string; name: string; description: string; owner: string; owner_uuid: string; visibility: 'public' | 'private'; version: string; downloads: number; stars: number; updated_at: string; tags: string[]; yaml: string };
export type Version = { version: string; commit: string; notes: string; published_at: string };
export type Discussion = { id: string; parent_id?: string; author: string; body: string; created_at: string };
export type RepoFile = { path: string; content: string | null; size?: number; reason?: string };
export type Block = { id: string; kind: 'gene' | 'isi' | 'function'; name: string; description: string; owner: string; owner_uuid: string; visibility: 'public' | 'private'; version: string; downloads: number; updated_at: string };
export type BlockContent = { id: string; kind: Block['kind']; version: string; text?: string; archive_base64?: string };
export type Recipe = { schema: number; name?: string; description?: string; variables: { name: string; type: string; prompt?: string; default: unknown; when?: string; choices?: unknown }[]; files: { from: string; to: string }[]; build: string[] };
export type Repository = { commit: string | null; files: RepoFile[]; draft?: boolean; truncated?: boolean; template?: Recipe | null; template_error?: string | null };
export type Route = { page: string; id: string; tab: string; path: string };
export const API = process.env.NEXT_PUBLIC_API_URL || '';
export const message = (error: unknown) => error instanceof Error ? error.message : String(error);
export const client = createApi(API);
export const api = client.request;
export const repoUrl = (id: string, tab = 'code', path = '') => `/starters/${encodeURIComponent(id)}${tab === 'code' ? '' : `/${tab}`}${path ? `/${path.split('/').map(encodeURIComponent).join('/')}` : ''}`;
export const blockUrl = (id: string) => `/blocks/${encodeURIComponent(id)}`;
export const blockLabel = (kind: string) => kind === 'isi' ? 'ISI' : kind === 'function' ? 'Function' : 'Gene';
export const downloadFile = (content: BlobPart, name: string, type: string) => {
  const url = URL.createObjectURL(new Blob([content], { type }));
  const anchor = document.createElement('a');
  anchor.href = url; anchor.download = name; anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
};
export const timeAgo = (value: string) => {
  const days = Math.max(0, Math.floor((Date.now() - new Date(value).getTime()) / 86400000));
  return days === 0 ? 'today' : days === 1 ? 'yesterday' : `${days} days ago`;
};
export const sizeLabel = (file: RepoFile) => {
  const bytes = file.size ?? (file.content === null ? null : new TextEncoder().encode(file.content).length);
  return bytes === null ? 'Size unavailable' : bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KB`;
};
export const Icon = (props: { name: string; size?: number }) => {
  if (props.name === 'gene') return <Dna size={props.size || 18} strokeWidth={1.7} aria-hidden="true" />;
  if (props.name === 'isi') return <Bot size={props.size || 18} strokeWidth={1.7} aria-hidden="true" />;
  const paths: Record<string, string> = {
    code: 'm8 7-5 5 5 5m8-10 5 5-5 5m-3-14-2 18', folder: 'M3 7V5h7l2 2h9v13H3V7Z', file: 'M6 3h8l4 4v14H6V3Zm8 0v5h4',
    star: 'm12 3 2.8 5.67 6.2.9-4.5 4.4 1.06 6.2L12 18.25 6.44 21.17 7.5 14.97 3 10.57l6.2-.9L12 3Z',
    architecture: 'M9 3h6v6H9V3ZM3 15h6v6H3v-6Zm12 0h6v6h-6v-6ZM12 9v3m-6 3v-3h12v3',
    releases: 'M3 3h8l10 10-8 8L3 11V3Zm4 4h.01', discussions: 'M4 4h16v12H9l-5 4V4Z',
    download: 'M12 3v12m0 0 5-5m-5 5-5-5M4 18v3h16v-3', search: 'M11 19a8 8 0 1 1 0-16 8 8 0 0 1 0 16Zm6-2 4 4',
    book: 'M4 5a2 2 0 0 1 2-2h14v18H6a2 2 0 0 1-2-2V5Zm0 12h16', plus: 'M12 5v14M5 12h14', copy: 'M9 9h12v12H9V9ZM3 15V3h12', template: 'M4 4h6v6H4V4Zm10 0h6v6h-6V4ZM4 14h6v6H4v-6Zm10 3h6m-3-3v6',
  };
  return <svg width={props.size || 18} height={props.size || 18} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[props.name] || paths.file} /></svg>;
};
export function ErrorMessage(props: { error: unknown; retry?: () => void }) {
  return <div className="notice error" role="alert">{message(props.error)}{props.retry && <Button variant="secondary" onClick={() => props.retry?.()}>Try again</Button>}</div>;
}

export function useResource<T>(path: string | false) {
  const [state, setState] = useState<{ path: string | false; data?: T; error?: unknown; loading: boolean }>({ path, loading: !!path });
  const pending = useRef<AbortController | null>(null);
  const refetch = useCallback(async () => {
    pending.current?.abort();
    if (!path) { setState({ path, loading: false }); return; }
    const controller = new AbortController();
    pending.current = controller;
    setState(previous => ({ path, data: previous.path === path ? previous.data : undefined, loading: true }));
    try {
      const data = await api<T>(path, { signal: controller.signal });
      if (!controller.signal.aborted) setState({ path, data, loading: false });
      return controller.signal.aborted ? undefined : data;
    } catch (error) {
      if (!controller.signal.aborted) setState({ path, error, loading: false });
    }
  }, [path]);
  useEffect(() => {
    void refetch();
    return () => pending.current?.abort();
  }, [refetch]);
  const mutate = useCallback((data: T) => {
    pending.current?.abort();
    setState({ path, data, loading: false });
  }, [path]);
  const current = state.path === path ? state : { data: undefined, error: undefined, loading: !!path };
  return { data: current.data, error: current.error, loading: current.loading, refetch, mutate };
}
