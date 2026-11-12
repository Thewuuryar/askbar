// Run (from clients/desktop): node tests/csp.test.js   (needs `playwright` with Chromium installed)
// The popup under the real content security policy from src-tauri/tauri.conf.json (ADR 0044, D17): no remote source, Trusted Types
// required with the one policy, and the real ui/ files working under it. Chromium stands in for WebView2; macOS (WKWebView) and
// Linux (WebKitGTK) are on the person-run checklist (ADR 0002), because the webview there is not Chromium.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const payloads = require('../../../tests/payloads/lib.js');

const ROOT = path.resolve(__dirname, '..');
const CSP = JSON.parse(fs.readFileSync(path.join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8')).app.security.csp;
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css' };

let failures = 0;
async function step(name, fn) {
  try { await fn(); console.log('ok   ' + name); }
  catch (e) { failures++; console.error('FAIL ' + name + '\n     ' + String(e.message).split('\n').join('\n     ')); }
}

const directives = (csp) => Object.fromEntries(csp.split(';').map((d) => d.trim()).filter(Boolean).map((d) => { const [n, ...v] = d.split(/\s+/); return [n, v]; }));

/** The popup's files, with `csp` as the policy header (none if null). */
function serveUi(csp) {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const file = path.join(ROOT, 'ui', req.url === '/' ? 'index.html' : req.url.split('?')[0]);
      if (!file.startsWith(path.join(ROOT, 'ui')) || !fs.existsSync(file)) { res.writeHead(404); res.end(); return; }
      const headers = { 'content-type': TYPES[path.extname(file)] || 'application/octet-stream' };
      if (csp) headers['content-security-policy'] = csp;
      res.writeHead(200, headers);
      res.end(fs.readFileSync(file));
    }).listen(0, '127.0.0.1', () => resolve({ server, url: 'http://127.0.0.1:' + server.address().port + '/' }));
  });
}

/** A server the popup must never reach: it keeps every request it gets. */
function decoy() {
  const seen = [];
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => { seen.push(req.url); res.writeHead(200, { 'access-control-allow-origin': '*' }); res.end('x'); })
      .listen(0, '127.0.0.1', () => resolve({ server, seen, url: 'http://127.0.0.1:' + server.address().port + '/' }));
  });
}

async function open(browser, url, init) {
  const ctx = await browser.newContext({ viewport: { width: 900, height: 760 } });
  await ctx.addInitScript(`window.__violations = []; document.addEventListener('securitypolicyviolation', (e) => window.__violations.push(e.violatedDirective + ' ' + e.blockedURI));`);
  if (init) await ctx.addInitScript(init);
  await ctx.addInitScript({ path: path.join(ROOT, 'tests/mock-tauri.js') });
  const page = await ctx.newPage();
  page.errors = [];
  page.on('pageerror', (e) => page.errors.push(e.message));
  page.on('console', (m) => { if (m.type() === 'error') page.errors.push(m.text()); });
  await page.goto(url);
  await page.waitForFunction(() => window.__calls && window.__calls.some((c) => c[0] === 'list_conversations'));
  return page;
}

const ask = async (page, text) => {
  await page.fill('#input', text);
  await page.press('#input', 'Enter');
  await page.waitForFunction(() => document.getElementById('stop').hidden && !document.querySelector('.msg.streaming'));
};

(async () => {
  // ------------------------------------------------------------------ the policy as written
  await step('the policy names no remote source, no inline or eval script, and requires Trusted Types with one policy', () => {
    const d = directives(CSP);
    for (const [name, sources] of Object.entries(d)) {
      for (const s of sources) {
        assert.ok(!/^(https?|wss?|ftp):\/\/(?!ipc\.localhost$)/i.test(s), `${name} allows a remote source: ${s}`);
        assert.notEqual(s, '*', `${name} allows any source`);
        assert.ok(!/^'unsafe-eval'$|^'wasm-unsafe-eval'$|^'unsafe-hashes'$/.test(s), `${name} allows ${s}`);
        if (/^(default|script)-src$/.test(name)) assert.notEqual(s, "'unsafe-inline'", `${name} allows inline script`);
      }
    }
    assert.deepEqual(d['default-src'], ["'self'"]);
    assert.ok(!d['script-src'] || !d['script-src'].includes("'unsafe-inline'"));
    assert.deepEqual(d['connect-src'], ['ipc:', 'http://ipc.localhost'], 'the only places it may connect are the bridge');
    assert.deepEqual(d['img-src'], ["'self'", 'data:'], 'no remote image');
    assert.deepEqual(d['require-trusted-types-for'], ["'script'"]);
    assert.deepEqual(d['trusted-types'], ['askbar-markdown'], 'one policy, no duplicates');
  });

  const browser = await chromium.launch();
  const remote = await decoy();
  const ui = await serveUi(CSP);

  // ------------------------------------------------------------------ the real files under it
  await step('the popup works under the real policy: replies, links and history render with no violation and no error', async () => {
    const page = await open(browser, ui.url);
    await ask(page, 'How do I format strings?');
    assert.ok(await page.locator('.msg.assistant a').count() >= 1, 'the reply has its link');
    assert.ok(await page.locator('.msg.assistant strong').count() >= 1, 'and its bold text');
    assert.ok(await page.locator('.msg.assistant pre code').count() >= 1, 'and its code block');
    await page.press('#input', 'Control+h');
    await page.locator('.conv .open').first().click();
    await page.waitForSelector('.msg.assistant a');
    assert.deepEqual(await page.evaluate(() => window.__violations), []);
    assert.deepEqual(page.errors, []);
    await page.context().close();
  });

  await step('every hostile payload under the real policy: no script, no request to anywhere else, no violation', async () => {
    const page = await open(browser, ui.url);
    const failuresIn = [];
    for (const c of payloads.cases()) {
      if (c.input.length > 100000) continue;
      await page.evaluate((r) => { window.__nextReply = r; window.__pwned = undefined; }, c.input);
      await page.press('#input', 'Control+n');
      await ask(page, 'go');
      if (await page.evaluate(() => window.__pwned !== undefined)) failuresIn.push(c.name + ': a script ran');
    }
    assert.deepEqual(failuresIn, []);
    assert.deepEqual(await page.evaluate(() => window.__violations), [], 'a reply caused a violation');
    assert.deepEqual(page.errors, []);
    assert.deepEqual(remote.seen, []);
    await page.context().close();
  });

  await step('Trusted Types is enforced: a string for an HTML sink, a second policy, a duplicate, a script and a string of code are all refused', async () => {
    const page = await open(browser, ui.url);
    const r = await page.evaluate(async (remoteUrl) => {
      const tryIt = (f) => { try { f(); return 'allowed'; } catch (e) { return e.name; } };
      const out = {};
      out.innerHTML = tryIt(() => { document.body.innerHTML = '<b>x</b>'; });
      out.insertAdjacentHTML = tryIt(() => document.body.insertAdjacentHTML('beforeend', '<b>x</b>'));
      out.other = tryIt(() => window.trustedTypes.createPolicy('other', { createHTML: (s) => s }));
      out.duplicate = tryIt(() => window.trustedTypes.createPolicy('askbar-markdown', { createHTML: (s) => s }));
      out.script = tryIt(() => { const s = document.createElement('script'); s.textContent = 'window.__pwned = 1'; document.head.appendChild(s); });
      // A string handed to setTimeout is code compiled later, outside this call, so the policy (not the debugger) decides.
      out.timer = await new Promise((resolve) => {
        try { setTimeout('window.__pwned = 2', 0); } catch (e) { resolve(e.name); return; }
        setTimeout(() => resolve(window.__pwned === undefined ? 'blocked' : 'ran'), 150);
      });
      out.fetch = await fetch(remoteUrl).then(() => 'allowed', () => 'blocked');
      out.image = await new Promise((resolve) => { const i = new Image(); i.onload = () => resolve('allowed'); i.onerror = () => resolve('blocked'); i.src = remoteUrl + 'pixel.png'; });
      out.pwned = window.__pwned;
      return out;
    }, remote.url);
    assert.deepEqual(r, { innerHTML: 'TypeError', insertAdjacentHTML: 'TypeError', other: 'TypeError', duplicate: 'TypeError', script: 'TypeError', timer: 'TypeError', fetch: 'blocked', image: 'blocked', pwned: undefined });
    assert.deepEqual(remote.seen, [], 'nothing reached the decoy server');
    await page.context().close();
  });

  await step('if the browser refuses the policy, a reply is shown as plain text: readable and never markup', async () => {
    const refused = await serveUi(CSP.replace('trusted-types askbar-markdown', 'trusted-types some-other-name'));
    const page = await open(browser, refused.url);
    await ask(page, 'How do I format strings?');
    const body = page.locator('.msg.assistant .body').first();
    assert.equal(await body.evaluate((n) => n.children.length), 0, 'no element was made');
    assert.match(await body.innerText(), /\*\*format strings\*\*/, 'the Markdown source is what shows');
    assert.equal(await page.isEnabled('#input'), true, 'and the popup still works');
    await page.context().close();
    refused.server.close();
  });

  await step('a webview with no Trusted Types and no such rule still renders replies through renderMarkdown', async () => {
    const plain = await serveUi(null);
    const page = await open(browser, plain.url, "Object.defineProperty(window, 'trustedTypes', { value: undefined, configurable: true });");
    await ask(page, 'How do I format strings?');
    assert.ok(await page.locator('.msg.assistant strong').count() >= 1);
    assert.ok(await page.locator('.msg.assistant a').count() >= 1);
    await page.evaluate((r) => { window.__nextReply = r; }, '<img src=x onerror="window.__pwned=1"> [x](javascript:window.__pwned=1)');
    await page.press('#input', 'Control+n');
    await ask(page, 'go');
    assert.equal(await page.locator('.msg.assistant img').count(), 0);
    assert.equal(await page.evaluate(() => window.__pwned), undefined);
    await page.context().close();
    plain.server.close();
  });

  await browser.close();
  ui.server.close();
  remote.server.close();
  console.log(failures ? `\n${failures} step(s) failed` : '\nall CSP steps passed');
  process.exit(failures ? 1 : 0);
})();
