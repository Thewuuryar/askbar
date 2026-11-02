// Run: node tests/audit-shots.js [outdir]   (needs the server and admin built, and playwright with Chromium)
// Not part of `npm test`. Starts the real server, admin and mock LLM, makes some traffic (a plain
// reply, one that needed a retry, one that fell back, a failure, one with a long provider name),
// then opens the Audit tab in Chromium at desktop and phone width, screenshots every view and
// checks each for sideways overflow and unreadably small text. Prints what it finds; exits 1 if the
// page itself scrolls sideways or text is smaller than 11px.
const { chromium } = require('playwright');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');

const ROOT = path.resolve(__dirname, '..');
const OUT = process.argv[2] || path.join(os.tmpdir(), 'audit-shots');
const PASSWORD = 'correct horse battery';
const AUDIT_PASS = 'correct horse battery staple 42';

const freePort = () => new Promise((resolve) => { const s = net.createServer().listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => resolve(p)); }); });
async function waitForPort(port) {
  for (let i = 0; i < 100; i++) {
    const ok = await new Promise((r) => { const c = net.connect(port, '127.0.0.1', () => { c.end(); r(true); }); c.on('error', () => r(false)); });
    if (ok) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error('port ' + port + ' never opened');
}

(async () => {
  fs.mkdirSync(OUT, { recursive: true });
  const [api, adminPort, serverApiPort, mockPort] = [await freePort(), await freePort(), await freePort(), await freePort()];
  const data = fs.mkdtempSync(path.join(os.tmpdir(), 'askbar-shots-'));
  const d = { serverData: path.join(data, 'data'), serverSecrets: path.join(data, 'data', 'secrets'), adminData: path.join(data, 'admin-data'), adminSecrets: path.join(data, 'admin-secrets'), serverToken: path.join(data, 'serverToken'), run: path.join(data, 'run') };
  fs.mkdirSync(d.run, { recursive: true });
  const sock = path.join(d.run, 'admin.sock');
  const children = [];
  const run = (cmd, args, env) => { const c = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: 'ignore' }); children.push(c); return c; };
  let forwarder;
  const cleanup = () => { for (const c of children) c.kill(); if (forwarder) forwarder.close(); fs.rmSync(data, { recursive: true, force: true }); };
  process.on('exit', cleanup);

  run('python3', ['-u', path.join(ROOT, 'tests/mock_llm.py'), String(mockPort)], {});
  run(path.join(ROOT, 'target/debug/askbar-admin'), [], { ASKBAR_ADMIN_SOCKET: sock, ASKBAR_ADMIN_DATA_DIR: d.adminData, ASKBAR_ADMIN_SECRETS_DIR: d.adminSecrets, ASKBAR_SERVER_TOKEN_DIR: d.serverToken, ASKBAR_SERVER_API_URL: 'http://127.0.0.1:' + serverApiPort, ASKBAR_PUBLIC_URL: 'http://localhost:' + api });
  for (let i = 0; i < 100 && !fs.existsSync(sock); i++) await new Promise((r) => setTimeout(r, 50));
  forwarder = net.createServer((c) => { const u = net.connect(sock); c.pipe(u); u.pipe(c); c.on('error', () => u.destroy()); u.on('error', () => c.destroy()); }).listen(adminPort, '127.0.0.1');
  run(path.join(ROOT, 'target/debug/askbar-server'), [], { ASKBAR_CLIENT_API_ADDR: '127.0.0.1:' + api, ASKBAR_SERVER_API_ADDR: '127.0.0.1:' + serverApiPort, ASKBAR_SERVER_TOKEN_HASH_FILE: path.join(d.serverToken, 'token.sha256'), ASKBAR_DATA_DIR: d.serverData, ASKBAR_SECRETS_DIR: d.serverSecrets, ASKBAR_AUDIT_DIR: path.join(data, 'audit') });
  await Promise.all([waitForPort(adminPort), waitForPort(api), waitForPort(mockPort)]);

  const base = `http://127.0.0.1:${adminPort}`;
  const H = { 'x-askbar-admin': '1', 'content-type': 'application/json' };
  let cookie = '';
  const admin = async (method, p, body) => {
    const r = await fetch(base + p, { method, headers: { ...H, cookie }, body: body === undefined ? undefined : JSON.stringify(body) });
    const sc = r.headers.get('set-cookie'); if (sc) cookie = sc.split(';')[0];
    const t = await r.text();
    return { status: r.status, body: t ? JSON.parse(t) : null };
  };
  await admin('PUT', '/admin/api/v1/setup/password', { password: PASSWORD });
  await admin('PUT', '/admin/api/v1/setup/audit-key', { passphrase: AUDIT_PASS, confirm: AUDIT_PASS });
  const mock = `http://127.0.0.1:${mockPort}/v1`;
  const mk = (name, extra = {}) => admin('POST', '/admin/api/v1/providers', { name, type: 'openai', base_url: mock, model: 'mock-model', api_key: 'sk-x', ...extra });
  await mk('Primary mock', { price_in: 2.5, price_out: 10, allow_as_fallback: true });
  await mk('Backup provider with a really quite long display name to test wrapping', { model: 'm-fallback-model-with-a-long-name', price_in: 1, price_out: 4, allow_as_fallback: true });
  const providers = (await admin('GET', '/admin/api/v1/providers')).body;
  const pid = (n) => providers.find((p) => p.name.startsWith(n)).id;
  await admin('PUT', '/admin/api/v1/settings', { system_prompt: 'x', display: (await admin('GET', '/admin/api/v1/settings')).body.display });

  const pairing = (await admin('POST', '/admin/api/v1/pairings', { name: 'shots' })).body.pairing;
  const { code } = JSON.parse(Buffer.from(pairing.slice('askbar-pair:'.length), 'base64url').toString());
  const h = { 'content-type': 'application/json', 'x-askbar-client-version': '0.4.0' };
  const { token } = await (await fetch(`http://127.0.0.1:${api}/api/v1/pair`, { method: 'POST', headers: h, body: JSON.stringify({ code }) })).json();
  const chat = async (provider, text) => (await fetch(`http://127.0.0.1:${api}/api/v1/chat`, { method: 'POST', headers: { ...h, authorization: 'Bearer ' + token }, body: JSON.stringify({ provider, text }) })).text();
  await chat(pid('Primary'), 'hello there');
  await chat(pid('Primary'), 'a cached prompt about caching');
  await chat(pid('Primary'), 'hello 429once screenshot');
  await chat(pid('Primary'), 'please fail');
  await chat(pid('Primary'), 'another plain one');
  await new Promise((r) => setTimeout(r, 600));

  const browser = await chromium.launch();
  const findings = [];
  for (const [label, width, height] of [['desktop', 1100, 800], ['phone', 390, 800]]) {
    const ctx = await browser.newContext({ viewport: { width, height } });
    const page = await ctx.newPage();
    await page.goto(base + '/admin/');
    await page.fill('#password', PASSWORD);
    await page.click('#login-form button');
    await page.waitForSelector('#app:not([hidden])');
    await page.click('#nav [data-tab="audit"]');
    await page.waitForFunction(() => Number(document.querySelector('[data-stat="requests"]')?.textContent) >= 1);
    const check = async (view) => {
      await page.waitForTimeout(250);
      const r = await page.evaluate(() => {
        const out = { sideways: document.documentElement.scrollWidth - window.innerWidth, small: [], clipped: [] };
        for (const n of document.querySelectorAll('#tab-audit *')) {
          if (n.offsetParent === null || !n.childNodes.length || ![...n.childNodes].some((c) => c.nodeType === 3 && c.textContent.trim())) continue;
          const fs = parseFloat(getComputedStyle(n).fontSize);
          if (fs < 11) out.small.push(n.tagName + ':' + n.textContent.trim().slice(0, 30) + ' ' + fs + 'px');
        }
        for (const n of document.querySelectorAll('#tab-audit .table-wrap')) if (n.scrollWidth > n.clientWidth + 1) out.clipped.push(n.querySelector('table')?.id + ' scrolls inside its box by ' + (n.scrollWidth - n.clientWidth) + 'px');
        return out;
      });
      await page.screenshot({ path: path.join(OUT, `${label}-${view}.png`), fullPage: true });
      if (r.sideways > 1) findings.push(`${label}/${view}: the page scrolls sideways by ${r.sideways}px`);
      for (const s of r.small) findings.push(`${label}/${view}: small text ${s}`);
      for (const c of r.clipped) findings.push(`${label}/${view}: (note) ${c}`);
    };
    await check('overview');
    await page.click('#audit-nav [data-sub="requests"]');
    await page.waitForSelector('#audit-rows tr');
    await check('requests');
    await page.click('#audit-rows tr:nth-child(3)'); // the 429-retry one
    await page.waitForSelector('#audit-waterfall .span-bar');
    await page.focus('#audit-waterfall .span-bar >> nth=-1');
    await check('trace');
    await page.click('#audit-nav [data-sub="routing"]');
    await page.waitForSelector('#audit-attempt-rows tr');
    await check('routing');
    await page.click('#audit-nav [data-sub="content"]');
    await page.waitForSelector('#audit-sealed .view-btn');
    await check('content');
    await page.click('#audit-sealed .view-btn');
    await page.fill('#audit-pass', AUDIT_PASS);
    await page.click('#audit-pass-ok');
    await page.waitForFunction(() => document.getElementById('audit-plain').textContent.length > 20);
    await page.screenshot({ path: path.join(OUT, `${label}-content-opened.png`) });
    await page.click('#audit-pass-cancel');
    await ctx.close();
  }
  await browser.close();
  console.log('screenshots in ' + OUT);
  const real = findings.filter((f) => !f.includes('(note)'));
  for (const f of findings) console.log((f.includes('(note)') ? 'note  ' : 'FOUND ') + f);
  console.log(real.length ? `\n${real.length} problem(s)` : '\nno overflow or tiny text found');
  process.exit(real.length ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
