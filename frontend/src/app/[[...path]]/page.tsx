import { Suspense } from 'react';
import StarterApp from '../../app-client';

export default function Page() {
  return <Suspense fallback={<div className="page-width loading">Loading Starter…</div>}><StarterApp /></Suspense>;
}
