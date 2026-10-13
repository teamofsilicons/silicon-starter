import type { Metadata, Viewport } from 'next';
import '../silicon-ui/foundation.css';
import '../styles.css';
import '../refinement.css';

export const metadata: Metadata = {
  title: 'Silicon Starter',
  description: 'Explore silicon architectures and reusable blocks published by Carbons and Silicons.',
  referrer: 'no-referrer',
};
export const viewport: Viewport = { themeColor: '#ffffff' };

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return <html lang="en" data-accent="neutral"><body>{children}</body></html>;
}
