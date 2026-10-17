import type { Metadata, Viewport } from 'next';
import localFont from 'next/font/local';
import '../silicon-ui/foundation.css';
import '../styles.css';
import '../refinement.css';

const bodyFont = localFont({ src: [{ path: './fonts/BDOGrotesk-Regular.woff2', weight: '400' }, { path: './fonts/BDOGrotesk-Medium.woff2', weight: '500' }, { path: './fonts/BDOGrotesk-Bold.woff2', weight: '700' }], variable: '--font-bdo', display: 'swap' });
const displayFont = localFont({ src: './fonts/HedvigLettersSerif-Regular.woff2', weight: '400', variable: '--font-hedvig', display: 'swap' });

export const metadata: Metadata = {
  title: 'Silicon Starter',
  description: 'Discover starters, genes, ISIs, and functions. The shared registry for everything you need to build your next silicon.',
  referrer: 'no-referrer',
};
export const viewport: Viewport = { themeColor: '#fffdf9' };

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return <html lang="en" className={`${bodyFont.variable} ${displayFont.variable}`} data-corners="round"><body>{children}</body></html>;
}
