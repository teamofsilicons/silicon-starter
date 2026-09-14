// Run after npm run build: node src/browser-check.mjs
// Uses only an isolated localhost mock; never creates records in a real registry.
import { createServer } from 'node:http';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, extname } from 'node:path';
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';

const dist = fileURLToPath(new URL('../dist/', import.meta.url));
const profile = await mkdtemp(join(tmpdir(), 'starter-browser-check-'));
let chrome;
let socket;
let listed = false;
let authenticated = false;
let registryError = false;
let created;
const requests = [];
const starter = { id: 'test.repo', name: 'Example architecture', owner: 'test', description: 'An organization-published architecture with real repository files.', visibility: 'public', version: '1.2', downloads: 12, stars: 3, updated_at: new Date().toISOString(), tags: ['rust', 'agents'], yaml: 'silicon:\n  id: example:test\n' };
const files = [
  { path: 'README.md', content: '# Example architecture\n\nBuild an organization-owned silicon.\n\n## Getting started\n\n- [Source](src/main.rs)\n- [Unsafe link](javascript:alert(1))\n\n```sh\nstarter pull test.repo\n```\n' },
  { path: 'src/main.rs', content: 'fn main() {\n    println!("hello");\n}\n' },
  { path: 'src/nested/config.yaml', content: 'enabled: true\n' },
  { path: '.github/workflows/test.yaml', content: 'name: Check\n' },
  { path: 'docs/a b+ç.md', content: '# Encoded filename\n' },
  { path: 'silicon.yaml', content: starter.yaml },
  { path: 'logo.png', content: null, reason: 'binary', size: 100 },
  { path: 'external-link', content: null, reason: 'symlink', size: 5 },
  { path: 'empty.txt', content: '', size: 0 },
];
const server = createServer(async (request, response) => {
  try {
    const path = new URL(request.url, 'http://localhost').pathname;
    requests.push(path);
    const json = (data, status = 200) => { response.writeHead(status, { 'Content-Type': 'application/json' }); response.end(JSON.stringify(data)); };
    if (path === '/auth/session') return json(authenticated ? { authenticated: true, org_id: 'test', org_ids: ['test', 'other'], actor: { name: 'Test member' } } : { authenticated: false });
    if (path === '/api/v1/starters' && request.method === 'POST') {
      let body = ''; for await (const chunk of request) body += chunk;
      created = JSON.parse(body); return json({ ...starter, ...created, owner: created.org_id }, 201);
    }
    if (path === '/api/v1/starters') return registryError ? json({ error: 'Mock registry unavailable' }, 503) : json(listed ? [starter] : []);
    const route = /^\/api\/v1\/starters\/([^/]+)(?:\/(.+))?$/.exec(path);
    if (route) {
      const [, id, endpoint] = route;
      if (id === 'test.missing') return json({ error: 'Starter not found' }, 404);
      if (!endpoint) return json({ ...starter, id });
      if (endpoint === 'versions') return json(id === 'test.draft' ? [] : [{ version: '1.2', commit: 'a12b3456789abcdef', notes: 'First published architecture.', published_at: starter.updated_at }]);
      if (endpoint === 'discussions') return json([{ id: 'one', author: 'test-member', body: 'How can I customize this?', created_at: starter.updated_at }]);
      if (endpoint === 'files') {
        if (id === 'test.broken') return json({ error: 'repository bundle or commit is invalid' }, 422);
        return json({ commit: id === 'test.draft' ? null : 'a12b3456789abcdef', draft: id === 'test.draft', truncated: false, files: id === 'test.empty' ? [] : id === 'test.draft' ? [{ path: 'silicon.yaml', content: starter.yaml }] : files });
      }
      return json({ error: 'Unmocked API call' }, 400);
    }
    if (path.startsWith('/api/') || path.startsWith('/auth/')) return json({ error: 'Unmocked API call' }, 404);
    const file = path.startsWith('/assets/') ? join(dist, path) : join(dist, 'index.html');
    response.writeHead(200, { 'Content-Type': ({ '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html' })[extname(file)] });
    response.end(await readFile(file));
  } catch (error) { response.writeHead(500); response.end(String(error)); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const pending = new Map();
let sequence = 0;
let sessionId;
const send = (method, params = {}) => new Promise((resolve, reject) => {
  const id = ++sequence;
  pending.set(id, { resolve, reject });
  socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
});
const execute = async expression => {
  const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
  return result.result.value;
};
const waitFor = async expression => {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (await execute(expression).catch(() => false)) return;
    await pause(100);
  }
  throw new Error(`Timed out waiting for ${expression}`);
};
const browser = async (...args) => {
  const [command, ...rest] = args;
  if (command === 'open') {
    await send('Page.navigate', { url: rest[0] });
    return waitFor(`location.href === ${JSON.stringify(rest[0])} && document.readyState === 'complete' && !!document.querySelector('.app')`);
  }
  if (command === 'eval') return execute(rest[0]);
  if (command === 'wait') {
    if (rest[0] === '--fn') return waitFor(rest[1]);
    if (rest[0] === '--text') return waitFor(`document.body.innerText.includes(${JSON.stringify(rest[1])})`);
    if (rest[0] === '--url') return waitFor(`location.href.endsWith(${JSON.stringify(rest[1].replace(/^\*\*/, ''))})`);
  }
  if (command === 'set') return send('Emulation.setDeviceMetricsOverride', { width: +rest[1], height: +rest[2], deviceScaleFactor: 1, mobile: false });
  if (command === 'screenshot') {
    const result = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
    return writeFile(rest[0], Buffer.from(result.data, 'base64'));
  }
  if (command === 'fill') return execute(`(() => { const input = document.querySelector(${JSON.stringify(rest[0])}); input.value = ${JSON.stringify(rest[1])}; input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  if (command === 'close') return;
  throw new Error(`Unsupported browser action ${command}`);
};
const evaluate = async code => browser('eval', `(() => { ${code} })()`);
const check = async (condition, label) => evaluate(`if (!(${condition})) throw new Error(${JSON.stringify(label)}); return ${JSON.stringify(label)};`);
const open = async path => { await browser('open', origin + path); await browser('wait', '--fn', '!document.body.innerText.includes("Loading starter") && !document.body.innerText.includes("Reading repository") && !document.body.innerText.includes("Checking session")'); };
const click = async selector => { await evaluate(`document.querySelector(${JSON.stringify(selector)}).click();`); };

try {
  chrome = spawn(process.env.CHROME_BIN || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', 'about:blank'], { stdio: 'ignore' });
  let port;
  for (let attempt = 0; attempt < 100; attempt++) {
    port = await readFile(join(profile, 'DevToolsActivePort'), 'utf8').catch(() => '');
    if (port) break;
    await pause(100);
  }
  assert(port, 'Local Chrome failed to start; set CHROME_BIN to a Chrome executable');
  const [debugPort, websocketPath] = port.trim().split('\n');
  socket = new WebSocket(`ws://127.0.0.1:${debugPort}${websocketPath}`);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  socket.onmessage = event => {
    const packet = JSON.parse(event.data);
    if (pending.has(packet.id)) {
      const request = pending.get(packet.id); pending.delete(packet.id);
      packet.error ? request.reject(new Error(packet.error.message)) : request.resolve(packet.result);
    }
  };
  const target = await send('Target.createTarget', { url: 'about:blank' });
  const attached = await send('Target.attachToTarget', { targetId: target.targetId, flatten: true });
  sessionId = attached.sessionId;
  await send('Page.enable');
  await send('Browser.grantPermissions', { origin, permissions: ['clipboardReadWrite', 'clipboardSanitizedWrite'] });
  await open('/starters/test.repo');
  await check('document.querySelector(".repository-tabs a[aria-current=page]").textContent === "Code"', 'Code is the default tab');
  await check('document.querySelectorAll(".file-row").length === 8 && !document.querySelector(".file-view")', 'Root opens as a directory listing');
  await check('document.querySelector(".markdown h1").textContent === "Example architecture" && !document.querySelector("a[href^=javascript]")', 'README preview is rendered safely');
  await check('!document.querySelector("[role=dialog],.modal-backdrop,.modal-close") && getComputedStyle(document.querySelector(".repository-body")).position !== "fixed"', 'Repository is a normal page');
  assert(!requests.includes('/api/v1/starters'), 'Direct repository route must not depend on the search list');
  await browser('set', 'viewport', '1440', '1000');
  await browser('screenshot', '/tmp/starter-code-desktop.png');
  await click('.file-row[href$="/tree/src"]');
  await check('location.pathname.endsWith("/tree/src") && document.querySelectorAll(".file-row").length === 3', 'Folder navigation');
  await click('.file-row[href$="/blob/src/main.rs"]');
  await check('location.pathname.endsWith("/blob/src/main.rs") && document.querySelectorAll(".line-number").length === 4', 'File navigation and line numbers');
  await click('.file-view header button');
  await browser('wait', '--text', 'File copied.');
  assert.equal(await execute('navigator.clipboard.readText()'), files[1].content);
  await browser('open', origin + '/starters/test.repo/blob/src/main.rs');
  await check('document.querySelector(".source-code").textContent.includes("println!")', 'File deep link reload');
  await click('.repository-tabs a[href$="/releases"]');
  await check('document.querySelector(".repository-tabs a[aria-current=page]").textContent.includes("Releases") && document.querySelector(".release")', 'Releases tab');
  await evaluate('history.back();');
  await browser('wait', '--fn', 'location.pathname.endsWith("/blob/src/main.rs")');
  await check('document.querySelector(".source-code")', 'Back restores file');
  await evaluate('history.forward();');
  await browser('wait', '--fn', 'location.pathname.endsWith("/releases")');
  await check('document.querySelector(".release")', 'Forward restores tab');
  await click('.repository-tabs a[href$="/architecture"]');
  await check('document.querySelector(".architecture-source").textContent.includes("example:test")', 'Architecture tab');
  await click('.repository-tabs a[href$="/discussions"]');
  await browser('wait', '--text', 'How can I customize this?');
  await check('document.querySelector(".comment").textContent.includes("test-member")', 'Discussion tab');
  await open('/starters/test.repo/blob/docs/a%20b%2B%C3%A7.md');
  await check('document.querySelector(".source-code").textContent.includes("Encoded filename")', 'Encoded file path');
  await open('/starters/test.repo/blob/logo.png');
  await check('document.body.innerText.includes("Binary file") && !document.querySelector(".source-code")', 'Binary placeholder');
  await open('/starters/test.repo/blob/empty.txt');
  await check('document.querySelector(".source-code") && document.body.innerText.includes("0 B")', 'Empty text file');
  await open('/starters/test.draft');
  await check('document.querySelector(".revision").textContent.includes("Draft") && document.querySelectorAll(".file-row").length === 1 && !document.querySelector(".readme")', 'Draft has only its real YAML');
  await open('/starters/test.broken');
  await check('document.querySelector("[role=alert]").textContent.includes("repository bundle") && !document.querySelector(".file-row")', 'Invalid bundle surfaces an error');
  await open('/starters/test.empty');
  await check('document.body.innerText.includes("No source files are available")', 'Empty committed tree');
  await open('/starters/test.missing');
  await check('document.querySelector("[role=alert]").textContent.includes("Starter not found")', 'Unknown starter');
  await open('/');
  await check('document.body.innerText.includes("No starters yet") && !document.querySelector(".starter-card")', 'Empty registry is not seeded');
  listed = true;
  await open('/');
  await click('.starter-card-footer a');
  await browser('wait', '--text', 'Repository files');
  await check('document.querySelector(".repository-tabs a[aria-current=page]").textContent === "Code"', 'View code opens root');
  await browser('set', 'viewport', '390', '844');
  await browser('screenshot', '/tmp/starter-code-mobile.png');
  await check('document.documentElement.scrollWidth <= window.innerWidth', 'Mobile has no horizontal overflow');
  registryError = true;
  await open('/');
  await check('document.querySelector("[role=alert]").textContent.includes("registry could not be loaded") && !document.querySelector(".starter-card")', 'Registry failure shows no fabricated fallback');
  await open('/new');
  await check('document.body.innerText.includes("Log in with an organization") && !document.querySelector(".create-form")', 'Guest cannot create');
  authenticated = true;
  await open('/new');
  await check('document.querySelectorAll(".create-form select:first-of-type option").length >= 2 && !document.querySelector(".create-form input").value', 'Verified organization selector and blank draft form');
  await evaluate('document.querySelector(".create-form select").value = "other"; document.querySelector(".create-form select").dispatchEvent(new Event("change", {bubbles:true}));');
  await browser('fill', '.starter-id-field input', 'new');
  await browser('fill', '.create-form > label:nth-of-type(3) input', 'New starter');
  await browser('fill', '.create-form textarea', 'silicon:\n  id: new:other\n');
  await click('.create-form button[type=submit], .create-form .primary');
  await browser('wait', '--url', '**/starters/other.new');
  assert.equal(created.org_id, 'other');
  assert.equal(created.id, 'other.new');
  console.log('PASS: Code default, root/tree/file links, reload, Back/Forward, all tabs, encoded paths, binary/empty/draft/error states, mobile, View code, empty registry, verified org creation.');
  console.log('Screenshots: /tmp/starter-code-desktop.png and /tmp/starter-code-mobile.png');
} finally {
  socket?.close();
  chrome?.kill();
  server.close();
  await pause(300);
  await rm(profile, { recursive: true, force: true });
}
