# Silicon UI in Starter

These are the native React components downloaded from the [official Silicon UI registry](https://ui.teamofsilicons.com): Button, Input, Textarea, Select, Card, Badge, and Dialog, with their complete CSS modules, shared motion presets, menu highlight, and foundation tokens. The registry source matches [Team of Silicons' repository at `7e7bf7a`](https://github.com/teamofsilicons/silicon-ui/tree/7e7bf7aff8d093c5fd14473698c1355d1218f526).

`../silicon-ui.tsx` re-exports the components without adapters. Each component retains its upstream `use client` boundary and native React API. Import `foundation.css` once in the root Next.js layout. Styling uses local CSS modules and tokens; no Tailwind, external scripts, remote fonts, or runtime registry requests are required.

Required packages: `motion`, `lucide-react`, `@radix-ui/react-dialog`, and `@radix-ui/react-select`, alongside React 19. Input and Textarea supply their own label and accept a required `label` prop. Select takes `label`, `options`, `value`, and `onValueChange`. Card takes a required `title` and optional description, children, footer content, and quick-look details.

The files retain their relative imports from the registry payloads. Update an item from `https://ui.teamofsilicons.com/r/<name>.json`, including its `registryDependencies` and bundled helper files; registry targets under `@components/silicon-ui/` map to this directory. The MIT notice, including both upstream copyright holders, is served at `/licenses/Silicon-UI.txt`.
