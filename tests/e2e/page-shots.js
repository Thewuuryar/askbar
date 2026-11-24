// Run: node tests/e2e/page-shots.js [outdir] [--docs]   (needs `cargo build` for target/debug, and playwright with Chromium)
//
// Not part of `npm test`. Boots the real stack (server, admin, auditor, model gateway, alerter, mock LLM) from
// target/debug, on a throwaway data directory, makes some traffic (a plain reply, one that needed a retry, one that
// fell back, a failure, a long provider name), signs in and visits every page in the admin's PAGE_SLUGS (read from
// /admin/constants.json) plus one reference article, at 1280 and 390 wide. Each visit saves one image:
// <outdir>/<slug with / as ->-1280.png and -390.png (default outdir: os.tmpdir()/page-shots, or the first argument).
//
// Each page must load (no HTTP error on the page or a 5xx from the admin, no JS exception, no #page-error message, the
// signed-in app shown), must not scroll sideways, and must not have text under SMALL_PX (the threshold the Audit
// screenshots used). The sideways and small-text checks run on every page and width. A table that scrolls inside its own
// box is a note, not a failure. The script exits 1 and prints every failing page, and exits 2 if the stack itself
// cannot start.
//
// --docs also saves four 1280-wide viewport shots into docs/reference/admin-ui/img/ for the reference pages. A PNG over
// DOCS_MAX_BYTES is saved as a JPEG instead, and the script says so. Commit those only after looking at them.
//
// Stray processes: the script kills the stack it started on exit (including on Ctrl-C). Check with
//   pgrep -af "target/debug/askbar-|page-shots"
// after a run; it should print nothing.
const { chromium } = require('playwright');
const { spawn, spawnSync, execFileSync } = require('node:child_process');
const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');
const { freePort } = require('./ports');

const ROOT = path.resolve(__dirname, '../..');
const args = process.argv.slice(2);
const DOCS = args.includes('--docs');
const OUT = path.resolve(args.find((a) => !a.startsWith('--')) || path.join(os.tmpdir(), 'page-shots'));
const DOCS_DIR = path.join(ROOT, 'docs/reference/admin-ui/img');
const DOCS_MAX_BYTES = 150 * 1024;
// The docs set: the page slug (or reference article) and the image name it is saved as.
const DOCS_SHOTS = [
  ['home', 'home'],
  ['audit/calls', 'audit-calls'],
  ['general', 'general'],
  ['reference/settings/network', 'reference-network'],
];
const REFERENCE_ARTICLE = 'reference/settings/network';
const SMALL_PX = 11;
const WIDTHS = [[1280, 800], [390, 844]];
const PASSWORD = 'correct horse battery';
const AUDIT_PASS = 'correct horse battery staple 42';
const BACKUP_PASS = 'shots backup passphrase, its own 1';

async function waitForPort(port) {
  for (let i = 0; i < 100; i++) {
    const ok = await new Promise((r) => { const c = net.connect(port, '127.0.0.1', () => { c.end(); r(true); }); c.on('error', () => r(false)); });
    if (ok) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error('port ' + port + ' never opened');
}

const children = [];
let forwarder;
let dataDir;
function stopStack() {
  for (const c of children) c.kill();
  children.length = 0;
  if (forwarder) { forwarder.close(); forwarder = undefined; }
  if (dataDir) {
    spawnSync('chmod', ['-R', 'u+rwx', dataDir]);
    fs.rmSync(dataDir, { recursive: true, force: true });
    dataDir = undefined;
  }
}
process.on('exit', stopStack);
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => process.exit(130));

// Boots the real stack and makes the traffic. Returns the admin's base URL (through the TCP forwarder) and the client API port.
async function bootStack() {
  const [api, adminPort, serverApiPort, mockPort] = [await freePort(), await freePort(), await freePort(), await freePort()];
  dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'askbar-page-shots-'));
  const data = dataDir;
  const d = { serverData: path.join(data, 'data'), serverSecrets: path.join(data, 'data', 'secrets'), adminData: path.join(data, 'admin-data'), adminSecrets: path.join(data, 'admin-secrets'), creds: path.join(data, 'credentials'), run: path.join(data, 'run') };
  for (const dir of [d.run, path.join(data, 'audit'), path.join(data, 'backup'), path.join(data, 'alerter-data'), path.join(data, 'cpa-config')]) fs.mkdirSync(dir, { recursive: true });
  execFileSync(path.join(ROOT, 'target/debug/askbar-bootstrap'), [], { env: { ...process.env, ASKBAR_BOOTSTRAP_ROOT: d.creds }, stdio: 'inherit' });
  const sock = path.join(d.run, 'admin.sock');
  const run = (cmd, argv, env) => { const c = spawn(cmd, argv, { env: { ...process.env, ...env }, stdio: 'ignore' }); children.push(c); return c; };

  const [gatewayAdminPort, gatewayServerPort, auditPort, alerterAuditorPort, alerterAdminPort, ntfyPort, smtpPort, smtpSeenPort, releasesPort, cpaPort] = await Promise.all(Array.from({ length: 10 }, freePort));
  run('python3', ['-u', path.join(ROOT, 'tests/e2e/mock_llm.py'), String(mockPort)], {});
  run(path.join(ROOT, 'target/debug/askbar-auditor'), [], { ASKBAR_AUDIT_API_ADDR: '127.0.0.1:' + auditPort, ASKBAR_AUDIT_DIR: path.join(data, 'audit'), ASKBAR_AUDITOR_TOKEN_DIR: path.join(d.creds, 'auditor-token') });
  run('python3', ['-u', path.join(ROOT, 'tests/e2e/ntfy_sink.py'), String(ntfyPort)], {});
  run('python3', ['-u', path.join(ROOT, 'tests/e2e/smtp_sink.py'), String(smtpPort), String(smtpSeenPort)], {});
  run('python3', ['-u', path.join(ROOT, 'tests/e2e/releases_mock.py'), String(releasesPort)], {});
  await waitForPort(releasesPort);
  run(path.join(ROOT, 'target/debug/askbar-alerter'), [], {
    ASKBAR_ALERTER_AUDITOR_ADDR: '127.0.0.1:' + alerterAuditorPort, ASKBAR_ALERTER_ADMIN_ADDR: '127.0.0.1:' + alerterAdminPort,
    ASKBAR_ALERTER_TOKEN_DIR: path.join(d.creds, 'alerter-token'), ASKBAR_ALERTER_DATA_DIR: path.join(data, 'alerter-data'),
    ASKBAR_ALERTER_RELEASES_URL: 'http://127.0.0.1:' + releasesPort + '/repos/Thewuuryar/askbar/releases/latest',
    ASKBAR_AUDIT_API_URL: 'http://127.0.0.1:' + auditPort,
  });
  run(path.join(ROOT, 'target/debug/askbar-model-gateway'), [], {
    ASKBAR_GATEWAY_SERVER_ADDR: '127.0.0.1:' + gatewayServerPort, ASKBAR_GATEWAY_ADMIN_ADDR: '127.0.0.1:' + gatewayAdminPort,
    ASKBAR_GATEWAY_TOKEN_DIR: path.join(d.creds, 'model-gateway-token'), ASKBAR_AUDIT_API_URL: 'http://127.0.0.1:' + auditPort,
  });
  run(path.join(ROOT, 'target/debug/askbar-admin'), [], {
    ASKBAR_ADMIN_SOCKET: sock, ASKBAR_ADMIN_DATA_DIR: d.adminData, ASKBAR_ADMIN_SECRETS_DIR: d.adminSecrets,
    ASKBAR_ADMIN_TOKEN_DIR: path.join(d.creds, 'admin-token'), ASKBAR_SERVER_API_URL: 'http://127.0.0.1:' + serverApiPort,
    ASKBAR_GATEWAY_URL: 'http://127.0.0.1:' + gatewayAdminPort, ASKBAR_AUDIT_API_URL: 'http://127.0.0.1:' + auditPort,
    ASKBAR_ALERTER_URL: 'http://127.0.0.1:' + alerterAdminPort, ASKBAR_PUBLIC_URL: 'http://localhost:' + api,
    ASKBAR_BACKUP_DIR: path.join(data, 'backup'),
    ASKBAR_CPA_CONFIG_DIR: path.join(data, 'cpa-config'), ASKBAR_CPA_URL: 'http://127.0.0.1:' + cpaPort,
  });
  for (let i = 0; i < 100 && !fs.existsSync(sock); i++) await new Promise((r) => setTimeout(r, 50));
  forwarder = net.createServer((c) => { const u = net.connect(sock); c.pipe(u); u.pipe(c); c.on('error', () => u.destroy()); u.on('error', () => c.destroy()); }).on('error', (e) => { console.error('forwarder cannot listen on ' + adminPort + ': ' + e.message); process.exit(1); }).listen(adminPort, '127.0.0.1');
  run(path.join(ROOT, 'target/debug/askbar-server'), [], {
    ASKBAR_CLIENT_API_ADDR: '127.0.0.1:' + api, ASKBAR_SERVER_API_ADDR: '127.0.0.1:' + serverApiPort,
    ASKBAR_SERVER_TOKEN_HASH_FILE: path.join(d.creds, 'server-token', 'accept', 'admin-server.sha256'),
    ASKBAR_DATA_DIR: d.serverData, ASKBAR_SECRETS_DIR: d.serverSecrets, ASKBAR_AUDIT_DIR: path.join(data, 'audit'),
    ASKBAR_SERVER_TOKEN_DIR: path.join(d.creds, 'server-token'), ASKBAR_GATEWAY_URL: 'http://127.0.0.1:' + gatewayServerPort, ASKBAR_AUDIT_API_URL: 'http://127.0.0.1:' + auditPort,
  });
  await Promise.all([waitForPort(adminPort), waitForPort(api), waitForPort(mockPort)]);

  // First-run setup over the admin API, so the browser lands in the app.
  const base = `http://127.0.0.1:${adminPort}`;
  const H = { 'x-askbar-admin': '1', 'content-type': 'application/json' };
  let cookie = '';
  const admin = async (method, p, body) => {
    const r = await fetch(base + p, { method, headers: { ...H, cookie }, body: body === undefined ? undefined : JSON.stringify(body) });
    const sc = r.headers.get('set-cookie'); if (sc) cookie = sc.split(';')[0];
    const t = await r.text();
    return { status: r.status, body: t ? JSON.parse(t) : null };
  };
  await admin('POST', '/api/admin/v1/session', { username: 'admin', password: 'admin' });
  await admin('PUT', '/api/admin/v1/password', { current: 'admin', new: PASSWORD });
  await admin('PUT', '/api/admin/v1/setup/audit-key', { passphrase: AUDIT_PASS, confirm: AUDIT_PASS });
  const bk = await admin('PUT', '/api/admin/v1/setup/backup-key', { passphrase: BACKUP_PASS, confirm: BACKUP_PASS });
  if (bk.status >= 300) console.warn('backup key setup returned ' + bk.status + ' (the Backup page may show its setup step)');
  const mock = `http://127.0.0.1:${mockPort}/v1`;
  const mk = (name, extra = {}) => admin('POST', '/api/admin/v1/providers', { name, type: 'openai', base_url: mock, model: 'mock-model', api_key: 'sk-x', ...extra });
  await mk('Primary mock', { price_in: 2.5, price_out: 10 });
  await mk('Backup provider with a really quite long display name to test wrapping', { model: 'm-backup-model-with-a-long-name', price_in: 1, price_out: 4 });
  const providers = (await admin('GET', '/api/admin/v1/providers')).body;
  const pid = (n) => providers.find((p) => p.name.startsWith(n)).id;
  // Nothing answers until a model is approved (ADR 0014).
  await admin('PUT', '/api/admin/v1/catalog', {
    models: [{ provider_id: pid('Primary'), model: 'mock-model' }, { provider_id: pid('Backup'), model: 'm-backup-model-with-a-long-name' }],
    default: pid('Primary') + '/mock-model',
  });
  await admin('PUT', '/api/admin/v1/settings', { system_prompt: 'x', display: (await admin('GET', '/api/admin/v1/settings')).body.display });
  await admin('POST', '/api/admin/v1/users', { name: 'shots' }).then(async (u) => {
    const pairing = (await admin('POST', '/api/admin/v1/users/' + u.body.id + '/pairing-keys', { name: 'shots' })).body.pairing;
    const { code } = JSON.parse(Buffer.from(pairing.slice('askbar-pair:'.length), 'base64url').toString());
    const h = { 'content-type': 'application/json', 'x-askbar-client-version': '0.6.0' };
    const { token } = await (await fetch(`http://127.0.0.1:${api}/api/client/v1/pairings`, { method: 'POST', headers: h, body: JSON.stringify({ code }) })).json();
    const spaces = await (await fetch(`http://127.0.0.1:${api}/api/client/v1/spaces`, { headers: { ...h, authorization: 'Bearer ' + token } })).json();
    const space = spaces.find((s) => s.kind === 'personal').id;
    const chat = async (provider, text) => (await fetch(`http://127.0.0.1:${api}/api/client/v1/spaces/${space}/chat`, { method: 'POST', headers: { ...h, authorization: 'Bearer ' + token }, body: JSON.stringify({ model: provider + '/mock-model', text }) })).text();
    // The catalog reaches the gateway, and the server's copy of it, within a few seconds.
    await new Promise((r) => setTimeout(r, 6000));
    await chat(pid('Primary'), 'hello there');                    // a plain reply
    await chat(pid('Primary'), 'a cached prompt about caching');
    await chat(pid('Primary'), 'hello 429once screenshot');       // needed a retry
    await chat(pid('Primary'), 'please fail');                    // fell back, then failed
    await chat(pid('Primary'), 'another plain one');
    await new Promise((r) => setTimeout(r, 600));
  });
  return { base };
}

// Visits one page and returns what is wrong with it (an empty list is a pass). Saves the image to `file`.
async function visit(page, base, slug, file, docsFile) {
  const problems = [];
  const errors = [];
  const onError = (e) => errors.push('JS exception: ' + e.message.split('\n')[0]);
  const onResponse = (r) => {
    const u = new URL(r.url());
    if (r.status() >= 500 && (u.pathname.startsWith('/admin/') || u.pathname.startsWith('/api/admin/'))) errors.push(`HTTP ${r.status()} from ${u.pathname}`);
  };
  page.on('pageerror', onError);
  page.on('response', onResponse);
  try {
    const url = `${base}/admin/${slug}/`;
    const res = await page.goto(url, { waitUntil: 'load', timeout: 15000 });
    if (!res || res.status() >= 400) problems.push(`HTTP ${res ? res.status() : 'no response'} for the page`);
    await page.waitForSelector('#app:not([hidden])', { timeout: 15000 }).catch(() => problems.push('the signed-in app is not shown'));
    await page.waitForLoadState('networkidle', { timeout: 5000 }).catch(() => {});
    await page.waitForTimeout(400);
    const r = await page.evaluate((small) => {
      const out = { pageError: '', sideways: document.documentElement.scrollWidth - window.innerWidth, small: [], clipped: [], appHidden: !document.getElementById('app') || document.getElementById('app').hidden };
      const pe = document.getElementById('page-error');
      if (pe && !pe.hidden && pe.textContent.trim()) out.pageError = pe.textContent.trim().slice(0, 120);
      for (const n of document.querySelectorAll('#app *')) {
        if (n.offsetParent === null || !n.childNodes.length || ![...n.childNodes].some((c) => c.nodeType === 3 && c.textContent.trim())) continue;
        const fs = parseFloat(getComputedStyle(n).fontSize);
        if (fs < small) out.small.push(n.tagName.toLowerCase() + ' "' + n.textContent.trim().slice(0, 30) + '" ' + fs + 'px');
      }
      for (const n of document.querySelectorAll('.table-wrap')) if (n.scrollWidth > n.clientWidth + 1) out.clipped.push((n.querySelector('table')?.id || 'a table') + ' scrolls inside its box by ' + (n.scrollWidth - n.clientWidth) + 'px');
      return out;
    }, SMALL_PX);
    if (r.appHidden && !problems.length) problems.push('the signed-in app is not shown');
    if (r.pageError) problems.push('page error shown: ' + r.pageError);
    if (r.sideways > 1) problems.push(`the page scrolls sideways by ${r.sideways}px`);
    for (const s of r.small) problems.push(`text smaller than ${SMALL_PX}px: ${s}`);
    problems.push(...errors);
    await page.screenshot({ path: file, fullPage: true });
    if (docsFile) {
      await page.screenshot({ path: docsFile.png, type: 'png' });
      const size = fs.statSync(docsFile.png).size;
      if (size > DOCS_MAX_BYTES) {
        fs.rmSync(docsFile.png);
        await page.screenshot({ path: docsFile.jpg, type: 'jpeg', quality: 80 });
        console.log(`  docs: ${path.basename(docsFile.png)} was ${(size / 1024).toFixed(0)} KB, saved ${path.basename(docsFile.jpg)} instead`);
      }
    }
    return { problems, notes: r.clipped };
  } catch (e) {
    problems.push('did not load: ' + String(e.message).split('\n')[0]);
    return { problems, notes: [] };
  } finally {
    page.off('pageerror', onError);
    page.off('response', onResponse);
  }
}

(async () => {
  const { base } = await bootStack();
  fs.mkdirSync(OUT, { recursive: true });
  if (DOCS) fs.mkdirSync(DOCS_DIR, { recursive: true });

  const browser = await chromium.launch();
  const failed = [];
  let shots = 0;
  let targets = [];
  try {
    for (const [width, height] of WIDTHS) {
      const label = String(width);
      const ctx = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 1 });
      const page = await ctx.newPage();
      await page.goto(base + '/admin/');
      await page.fill('#password', PASSWORD);
      await page.click('#login-form button');
      await page.waitForSelector('#app:not([hidden])', { timeout: 15000 });
      if (targets.length === 0) {
        // The page list comes from the running admin, not from a copy here.
        const constants = await (await page.request.get(base + '/admin/constants.json')).json();
        const slugs = constants.PAGE_SLUGS;
        if (!Array.isArray(slugs) || slugs.length === 0) throw new Error('/admin/constants.json has no PAGE_SLUGS');
        targets = [...slugs, REFERENCE_ARTICLE];
        console.log(`${slugs.length} pages from constants.json, and the reference article ${REFERENCE_ARTICLE}`);
      }
      for (const slug of targets) {
        const name = slug.replace(/\//g, '-');
        const file = path.join(OUT, `${name}-${label}.png`);
        const docs = DOCS && width === 1280 && DOCS_SHOTS.find(([s]) => s === slug);
        const docsFile = docs ? { png: path.join(DOCS_DIR, `${docs[1]}.png`), jpg: path.join(DOCS_DIR, `${docs[1]}.jpg`) } : null;
        const { problems, notes } = await visit(page, base, slug, file, docsFile);
        shots++;
        for (const n of notes) console.log(`note  ${slug} at ${label}: ${n}`);
        if (problems.length) {
          failed.push(...problems.map((p) => `${slug} at ${label}: ${p}`));
          console.log(`FAIL  ${slug} at ${label}`);
        } else {
          console.log(`ok    ${slug} at ${label}`);
        }
      }
      await ctx.close();
    }
  } finally {
    await browser.close();
  }
  console.log(`\n${shots} screenshot(s) in ${OUT}`);
  if (failed.length) {
    console.log('\nProblems:');
    for (const f of failed) console.log('  ' + f);
    console.log(`\n${failed.length} problem(s)`);
    process.exit(1);
  }
  console.log('all pages loaded, none scrolls sideways, no text under ' + SMALL_PX + 'px');
  process.exit(0);
})().catch((e) => { console.error(e); process.exit(2); });
