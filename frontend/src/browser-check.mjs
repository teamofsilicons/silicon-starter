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
let selectedContext = 'test-context';
const contexts = [{context_id:'test-context',org_id:'test',actor:{type:'carbon',public_id:'c:test_member',name:'Test member'}},{context_id:'other-context',org_id:'other',actor:{type:'carbon',public_id:'c:test_member',name:'Test member'}}];
const selected = () => contexts.find(c=>c.context_id===selectedContext);
let registryError = false;
let created;
const requests = [];
const callbacks = [];
const callbackReferrers = [];
let callbackFailures = 0;
let permissionCompleted = false;
let permissionUnavailable = false;
const permissionKeys = [];
const starter = { id: 'test.repo', name: 'Example architecture', owner: 'test', description: 'An organization-published architecture with real repository files.', visibility: 'public', version: '1.2', downloads: 12, stars: 3, updated_at: new Date().toISOString(), tags: ['rust', 'agents'], yaml: 'silicon:\n  id: si:example\n  org_id: test\n' };
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
const recipe = {
  schema: 1, name: 'Configurable silicon', description: 'Choose how your silicon works.', auto_update: true,
  variables: [
    { name: 'silicon_token', type: 'secret', prompt: 'What is your token?', default: '' },
    { name: 'timezone', type: 'string', default: '! node timezone.js !>> "UTC"' },
    { name: 'purpose', type: 'string', prompt: 'What is its focus?', default: '<img src=x onerror="window.templateExecuted=true">' },
    { name: 'waveform', type: 'boolean', prompt: 'Include voice?', default: true },
    { name: 'provider', type: 'select', prompt: 'Which provider?', when: '{var.waveform}', choices: '! sh providers.sh !>> ["google"]', default: 'google' },
    { name: 'count', type: 'number', default: 0 },
    { name: 'temperature', type: 'float', default: 0.5 },
    { name: 'days', type: 'multiselect', choices: ['monday', 'friday'], default: ['monday'] },
    { name: 'tasks', type: 'list', default: ['research', { topic: 'planning' }] },
    { name: 'style', type: 'object', default: { tone: 'concise' } },
  ],
  files: [{ from: 'silicon.yaml.tmpl', to: 'silicon.yaml' }], build: ['! sh build.sh'],
};
const recipeFiles = [
  ...files,
  { path: '.starterbase/starter.yaml', content: 'schema: 1\n' },
  { path: '.starterbase/silicon.yaml.tmpl', content: 'silicon:\n  timezone: {{ var.timezone | tojson }}\n' },
  { path: '.starterbase/build.sh', content: 'mkdir -p "$STARTER_OUTPUT/workspace"\n' },
  { path: '.starterbase/providers.sh', content: 'printf \'["google"]\\n\'\n' },
];
const server = createServer(async (request, response) => {
  try {
    const path = new URL(request.url, 'http://localhost').pathname;
    requests.push(path);
    const json = (data, status = 200) => { response.writeHead(status, { 'Content-Type': 'application/json' }); response.end(JSON.stringify(data)); };
    if (path === '/auth/session') return json(authenticated ? {authenticated:true,...selected(),org_ids:[selected().org_id]} : {authenticated:false});
    if(path === '/auth/contexts') return json({contexts:contexts.map(c=>({...c,selected:c.context_id===selectedContext}))});
    if(path === '/auth/context') {
      assert.equal(request.headers['x-starter-context'],selectedContext);
      let body=''; for await(const chunk of request) body+=chunk; selectedContext=JSON.parse(body).context_id; return json({});
    }
    if (path === '/auth/callback' && request.method === 'POST') {
      let body = ''; for await (const chunk of request) body += chunk;
      callbacks.push(JSON.parse(body)); callbackReferrers.push(request.headers.referer);
      if (callbackFailures-- > 0) return json({error:'Temporary IAM interruption'},503);
      authenticated = true; return json({ authenticated: true });
    }
    if (path.startsWith('/api/v1/briefcase/')) {
      assert.equal(request.headers['x-starter-context'],selectedContext);
      if (permissionUnavailable) return json({error:{code:'reconsent_required',message:'Review storage access again'}},403);
      if(path.endsWith('/authorization')) { permissionKeys.push(request.headers['idempotency-key']); permissionCompleted=false; }
      if(path.endsWith('/complete')) permissionCompleted=true;
      return json({request_id:'8d4d2a5d-0592-49af-81e0-799dd8d216ab',context_id:selectedContext,completed:permissionCompleted,roots:[],authorization:{id:'a3a262c3-ebed-4295-9756-55ee824f1e75',app_id:'starter',actor:selected().actor,org_id:selected().org_id,status:permissionCompleted?'exchanged':'pending',version:1,expires_at:'2099-01-01T00:00:00Z',state:null,authorization_url:'https://iam.example/review'}});
    }
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
        if (id === 'test.template-slow') await pause(800);
        if (id.startsWith('test.template')) return json({ commit: 'a12b3456789abcdef', files: recipeFiles, template: id === 'test.template-invalid' ? null : recipe, template_error: id === 'test.template-invalid' ? 'variable purpose is missing a default' : null });
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
    const previous = await execute('performance.timeOrigin');
    await send('Page.navigate', { url: rest[0] });
    return waitFor(`performance.timeOrigin !== ${previous} && location.href === ${JSON.stringify(rest[0])} && document.readyState === 'complete' && !!document.querySelector('.app')`);
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
const open = async path => { await browser('open', origin + path); await browser('wait', '--fn', '!!document.querySelector(".app") && !document.body.innerText.includes("Loading starter") && !document.body.innerText.includes("Reading repository") && !document.body.innerText.includes("Reading template") && !document.body.innerText.includes("Checking session")'); };
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
  await open('/starters/test.repo/blob/src/main.rs');
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
  await check('document.querySelector(".architecture-source").textContent.includes("si:example")', 'Architecture tab');
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
  await check('!document.querySelector(".repository-tabs a[href$=template]")', 'Plain starters have no template tab');
  await open('/starters/test.template/template');
  await check('document.querySelector(".repository-tabs a[aria-current=page]").textContent === "Template"', 'Template deep link selects the template tab');
  assert.deepEqual(await execute('[...document.querySelectorAll(".template-question")].map(item => item.dataset.variable)'), recipe.variables.map(variable => variable.name));
  await check('document.querySelectorAll(".template-flow > li").length === 5 && document.querySelector(".template-intro").textContent.includes("enabled")', 'Ordered setup and build flow');
  await check('document.querySelector("[data-variable=timezone]").textContent.includes("resolves silently") && document.querySelector("[data-variable=timezone]").textContent.includes("! node timezone.js")', 'Silent dynamic default is visible as source');
  await check('document.querySelector("[data-variable=provider]").textContent.includes("{var.waveform}") && document.querySelector("[data-variable=provider]").textContent.includes("! sh providers.sh")', 'Conditions and prepared choices remain visible');
  await check('document.querySelector("[data-variable=silicon_token]").textContent.includes("Input is masked") && document.querySelector("[data-variable=tasks]").textContent.includes("planning") && document.querySelector("[data-variable=style]").textContent.includes("concise")', 'Secret and structured answer types');
  await check('!document.querySelector(".template-panel input,.template-panel img,.template-panel script") && !window.templateExecuted', 'Preview renders values as text and does not execute author content');
  await browser('screenshot', '/tmp/starter-template-desktop.png');
  await click('.template-files a');
  await check('location.pathname.endsWith("/blob/.starterbase/silicon.yaml.tmpl") && document.querySelector(".source-code").textContent.includes("var.timezone")', 'Template source link opens its ingredient');
  await click('.repository-tabs a[href$="/template"]');
  await click('.template-ingredients header a');
  await check('location.pathname.endsWith("/tree/.starterbase") && document.querySelectorAll(".file-row").length === 5', 'Ingredients folder is browsable');
  await open('/starters/test.template-invalid/template');
  await check('document.querySelector("[role=alert]").textContent.includes("missing a default") && !document.querySelector(".template-flow") && document.querySelectorAll(".template-ingredients .file-row").length === 4', 'Invalid recipe shows its error and preserves source browsing');
  await browser('open', origin + '/starters/test.template-slow/template');
  await browser('wait', '--text', 'Reading template');
  await check('!document.querySelector(".template-flow")', 'Template loading does not show stale questions');
  await browser('wait', '--fn', '!!document.querySelector(".template-flow")');
  await open('/starters/test.broken/template');
  await check('document.querySelector("[role=alert]").textContent.includes("repository bundle") && document.querySelector("[role=alert] button") && !document.querySelector(".template-flow")', 'Template fetch failure offers retry');
  await open('/starters/test.draft/template');
  await check('document.body.innerText.includes("No template recipe") && !document.querySelector(".template-flow")', 'Template route handles plain starters');
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
  await open('/starters/test.template/template');
  await browser('screenshot', '/tmp/starter-template-mobile.png');
  await check('document.documentElement.scrollWidth <= window.innerWidth', 'Template has no mobile horizontal overflow');
  registryError = true;
  await open('/');
  await check('document.querySelector("[role=alert]").textContent.includes("registry could not be loaded") && !document.querySelector(".starter-card")', 'Registry failure shows no fabricated fallback');
  await open('/new');
  await check('document.body.innerText.includes("Log in with an organization") && !document.querySelector(".create-form")', 'Guest cannot create');
  const slt = 'oac_starter:opaque+value/=';
  const entries = [{ app_id: 'tos>starter', slt: 'oac_legacy' }, { app_id: 'iam', slt: 'oac_other' }, { app_id: 'starter', slt }];
  await send('Page.navigate', { url: `${origin}/?state=opaque-state#slts=${encodeURIComponent(JSON.stringify(entries))}` });
  await browser('wait', '--fn', 'location.search === "" && location.hash === "" && !!document.querySelector(".account-name")');
  assert.deepEqual(callbacks, [{ slt, state: 'opaque-state' }], 'Select the bare starter app and preserve its opaque SLT');
  callbackFailures=1;authenticated=false;
  await send('Page.navigate', {url:`${origin}/?slt=oac_retry&state=retry-state`});
  await browser('wait','--text','Temporary IAM interruption');
  await check('location.search.includes("oac_retry") && !document.querySelector(".account-name")', 'An uncertain callback remains recoverable without adopting another account');
  await click('nav a[href="/docs"]');
  await check('location.search.includes("oac_retry")', 'Internal navigation cannot discard a pending login callback');
  await check('document.querySelector("meta[name=referrer]").content === "no-referrer" && !JSON.stringify(localStorage).includes("oac_retry")', 'Callback is not persisted to localStorage or exposed as a referrer');
  await evaluate('[...document.querySelectorAll("button")].find(b=>b.textContent==="Retry login").click();');
  await browser('wait','--fn','location.search === "" && !!document.querySelector(".account-name")');
  assert.deepEqual(callbacks[1],callbacks[2],'Explicit retry preserves the original callback exactly');
  assert(callbackReferrers.every(value=>value===undefined),'Callback requests must not disclose the callback URL as Referer');
  await send('Page.navigate', { url: `${origin}/?state=legacy-state#slts=${encodeURIComponent(JSON.stringify(entries.slice(0, 2)))}` });
  await browser('wait', '--text', 'No login token for starter; log in again with IAM.');
  assert.equal(callbacks.length, 3, 'Legacy and unrelated app tokens must never be exchanged for Starter');
  await check('location.hash.startsWith("#slts=") && !document.querySelector(".account-name")', 'Rejected callback never adopts an existing session and remains cancellable');
  await evaluate('[...document.querySelectorAll("button")].find(b=>b.textContent==="Cancel login").click();');
  await browser('wait','--fn','location.search === "" && location.hash === "" && !!document.querySelector(".account-name")');
  await open('/permissions');
  await evaluate('[...document.querySelectorAll("button")].find(b=>b.textContent==="Review storage access").click();');
  await browser('wait','--text','Code from IAM');
  await browser('fill','.permission-card input','manual-code');
  await evaluate('document.querySelector(".permission-card form").requestSubmit();');
  await browser('wait','--text','Storage access is ready');
  await evaluate('[...document.querySelectorAll("button")].find(b=>b.textContent==="Review storage access again").click();');
  await evaluate('[...document.querySelectorAll("button")].find(b=>b.textContent==="Review storage access").click();');
  await browser('wait','--text','Code from IAM');
  assert.notEqual(permissionKeys[0],permissionKeys[1],'Completed review can start a fresh independent request');
  await evaluate('localStorage.setItem("starter:iam5:briefcase:other-context","keep-other-review");');
  permissionUnavailable=true;
  await evaluate('[...document.querySelectorAll("button")].find(b=>b.textContent==="Check status").click();');
  await browser('wait','--text','This permission needs a fresh review');
  await check('!document.querySelector(".permission-card input") && localStorage.getItem("starter:iam5:briefcase:other-context")==="keep-other-review"', 'Reconsent resets only the active review and retains other contexts');
  permissionUnavailable=false;
  authenticated = true;
  await open('/new');
  await check('document.querySelectorAll(".create-form select")[0].options.length === 1 && !document.querySelector(".create-form input").value', 'Verified organization selector and blank draft form');
  await evaluate('document.querySelector(".account-controls select").value = "other-context"; document.querySelector(".account-controls select").dispatchEvent(new Event("change", {bubbles:true}));');
  await browser('wait','--url',origin+'/');
  await open('/new');
  await check('document.querySelector(".create-form select").value === "other"', 'Saved context selects an independent organization login');
  await browser('fill', '.starter-id-field input', 'new');
  await browser('fill', '.create-form > label:nth-of-type(3) input', 'New starter');
  await browser('fill', '.create-form textarea', 'silicon:\n  id: si:new\n  org_id: other\n');
  await click('.create-form button[type=submit], .create-form .primary');
  await browser('wait', '--url', '**/starters/other.new');
  assert.equal(created.org_id, 'other');
  assert.equal(created.id, 'other.new');
  console.log('PASS: Code default, root/tree/file links, reload, Back/Forward, all tabs, template order/types/conditions/flow/source links/safe rendering, encoded paths, binary/empty/draft/error states, mobile, View code, empty registry, canonical IAM app selection, legacy login rejection, verified org creation.');
  console.log('Screenshots: /tmp/starter-code-desktop.png and /tmp/starter-code-mobile.png');
} finally {
  socket?.close();
  chrome?.kill();
  server.close();
  await pause(300);
  await rm(profile, { recursive: true, force: true });
}
