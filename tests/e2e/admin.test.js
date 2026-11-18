// Run: node tests/e2e/admin.test.js   (needs `cargo build -p askbar-server -p askbar-admin -p askbar-bootstrap` and `playwright` with Chromium)
// Starts the real server, the real admin service and tests/e2e/mock_llm.py, then drives the admin site in a browser.
// The admin listens on a unix socket only, so the test puts a small TCP forwarder in front of it for the browser.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const { spawn, spawnSync, execFileSync } = require('node:child_process');
const fs = require('node:fs');
const net = require('node:net');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');

const ROOT = path.resolve(__dirname, '../..');
const PASSWORD = 'correct horse battery';
const SECRET_KEY = 'sk-do-not-show-me-123';

// Every step runs once per viewport (plan step 1.7): the whole sequence, on a fresh stack and data directory, at 1280 wide
// and again at 390 wide. The names carry the viewport, so a failure says where it happened. The width is this test's own
// parameter, not a constants.json value.
const VIEWPORTS = [{ width: 1280, height: 800 }, { width: 390, height: 844 }];
const PHONE_MAX_WIDTH = 600;
let vpLabel = '';
let isPhone = false;
let failures = 0;
const counts = {};
async function step(name, fn) {
  const full = '[' + vpLabel + '] ' + name;
  try { await fn(); counts[vpLabel].ok++; console.log('ok   ' + full); }
  catch (e) { failures++; counts[vpLabel].fail++; console.error('FAIL ' + full + '\n     ' + String(e.message).split('\n').join('\n     ')); }
}

// Waits until something listens on the port: a process started a moment ago may not be yet.
const listening = async (port) => {
  for (let i = 0; i < 100; i++) {
    if (await new Promise((resolve) => { const c = net.connect(port, '127.0.0.1', () => { c.destroy(); resolve(true); }); c.on('error', () => resolve(false)); })) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error('nothing listens on port ' + port);
};
const freePort = () => new Promise((resolve) => {
  const s = net.createServer().listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => resolve(p)); });
});

async function waitForPort(port) {
  for (let i = 0; i < 100; i++) {
    const ok = await new Promise((r) => { const c = net.connect(port, '127.0.0.1', () => { c.end(); r(true); }); c.on('error', () => r(false)); });
    if (ok) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error('port ' + port + ' never opened');
}

async function runViewport(viewport) {
  vpLabel = String(viewport.width);
  isPhone = viewport.width <= PHONE_MAX_WIDTH;
  counts[vpLabel] = { ok: 0, fail: 0 };
  const [api, adminPort, serverApiPort, mockPort] = [await freePort(), await freePort(), await freePort(), await freePort()];
  const data = fs.mkdtempSync(path.join(os.tmpdir(), 'askbar-admin-'));
  const dirs = {
    serverData: path.join(data, 'data'), serverSecrets: path.join(data, 'data', 'secrets'),
    adminData: path.join(data, 'admin-data'), adminSecrets: path.join(data, 'admin-secrets'),
    creds: path.join(data, 'credentials'), run: path.join(data, 'admin-run'),
  };
  fs.mkdirSync(dirs.run, { recursive: true });
  fs.mkdirSync(path.join(data, 'audit'), { recursive: true });
  fs.mkdirSync(path.join(data, 'alerter-data'), { recursive: true });
  // The tokens come from bootstrap, as in the stack (ADR 0049); here its volumes are plain directories.
  const hashFile = path.join(dirs.creds, 'server-token', 'accept', 'admin-server.sha256');
  const bootstrapBin = path.join(ROOT, 'target/debug/askbar-bootstrap');
  if (!fs.existsSync(bootstrapBin)) execFileSync('cargo', ['build', '-q', '-p', 'askbar-bootstrap'], { cwd: ROOT, stdio: 'inherit' });
  execFileSync(bootstrapBin, [], { env: { ...process.env, ASKBAR_BOOTSTRAP_ROOT: dirs.creds }, stdio: 'inherit' });
  // The gateway holds the provider keys and does the model lists and provider tests (ADR 0054).
  const gatewayBin = path.join(ROOT, 'target/debug/askbar-model-gateway');
  if (!fs.existsSync(gatewayBin)) execFileSync('cargo', ['build', '-q', '-p', 'askbar-model-gateway'], { cwd: ROOT, stdio: 'inherit' });
  const gatewayAdminPort = await freePort();
  const [gatewayServerPort, auditPort] = [await freePort(), await freePort()];
  // The alerter delivers alerts (ADR 0055), here to a stand-in ntfy server and a stand-in mail server.
  const alerterBin = path.join(ROOT, 'target/debug/askbar-alerter');
  if (!fs.existsSync(alerterBin)) execFileSync('cargo', ['build', '-q', '-p', 'askbar-alerter'], { cwd: ROOT, stdio: 'inherit' });
  const [alerterAuditorPort, alerterAdminPort, ntfyPort, smtpPort, smtpSeenPort, releasesPort] = [await freePort(), await freePort(), await freePort(), await freePort(), await freePort(), await freePort()];
  const auditorBin = path.join(ROOT, 'target/debug/askbar-auditor');
  if (!fs.existsSync(auditorBin)) execFileSync('cargo', ['build', '-q', '-p', 'askbar-auditor'], { cwd: ROOT, stdio: 'inherit' });
  const LOGIN_MARKER = 'marker-account-login-3e91d7a2';
  // The supervisor from the `askbar-cpa` image answers the admin's export request (ADR 0013), here with a stand-in for CLIProxyAPI.
  const supervisorBin = path.join(ROOT, 'target/debug/askbar-cpa-supervisor');
  if (!fs.existsSync(supervisorBin)) execFileSync('cargo', ['build', '-q', '-p', 'askbar-cpa-supervisor'], { cwd: ROOT, stdio: 'inherit' });
  const sock = path.join(dirs.run, 'admin.sock');
  const children = [];
  const run = (cmd, args, env) => { const c = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: 'ignore' }); children.push(c); return c; };
  let forwarder = null;
  // bootstrap's directories are read-only, as they are in the stack: open them up before removing.
  const cleanup = () => { for (const c of children) c.kill(); if (forwarder) forwarder.close(); spawnSync('chmod', ['-R', 'u+rwx', data]); fs.rmSync(data, { recursive: true, force: true }); };
  let browser = null;
  try {

  const startServer = () => run(path.join(ROOT, 'target/debug/askbar-server'), [], {
    ASKBAR_CLIENT_API_ADDR: '127.0.0.1:' + api,
    ASKBAR_SERVER_API_ADDR: '127.0.0.1:' + serverApiPort, ASKBAR_SERVER_TOKEN_HASH_FILE: hashFile,
    ASKBAR_DATA_DIR: dirs.serverData, ASKBAR_SERVER_TOKEN_DIR: path.join(dirs.creds, 'server-token'),
    ASKBAR_GATEWAY_URL: 'http://127.0.0.1:' + gatewayServerPort, ASKBAR_AUDIT_API_URL: 'http://127.0.0.1:' + auditPort,
  });
  run('python3', ['-u', path.join(ROOT, 'tests/e2e/mock_llm.py'), String(mockPort)], {});
  // The auditor records every reply before a provider is called (ADR 0051), so the Audit tab has a real trail to show.
  run(auditorBin, [], {
    ASKBAR_AUDIT_API_ADDR: '127.0.0.1:' + auditPort, ASKBAR_AUDIT_DIR: path.join(data, 'audit'),
    ASKBAR_AUDITOR_TOKEN_DIR: path.join(dirs.creds, 'auditor-token'),
  });
  run('python3', ['-u', path.join(ROOT, 'tests/e2e/ntfy_sink.py'), String(ntfyPort)], {});
  run('python3', ['-u', path.join(ROOT, 'tests/e2e/smtp_sink.py'), String(smtpPort), String(smtpSeenPort)], {});
  run('python3', ['-u', path.join(ROOT, 'tests/e2e/releases_mock.py'), String(releasesPort)], {});
  // The alerter asks it as soon as it starts, and a check that fails is tried again only after an hour.
  await listening(releasesPort);
  run(alerterBin, [], {
    ASKBAR_ALERTER_AUDITOR_ADDR: '127.0.0.1:' + alerterAuditorPort, ASKBAR_ALERTER_ADMIN_ADDR: '127.0.0.1:' + alerterAdminPort,
    ASKBAR_ALERTER_TOKEN_DIR: path.join(dirs.creds, 'alerter-token'), ASKBAR_ALERTER_DATA_DIR: path.join(data, 'alerter-data'),
    ASKBAR_ALERTER_RELEASES_URL: 'http://127.0.0.1:' + releasesPort + '/repos/Thewuuryar/askbar/releases/latest',
    ASKBAR_AUDIT_API_URL: 'http://127.0.0.1:' + auditPort,
  });
  run(gatewayBin, [], {
    ASKBAR_GATEWAY_SERVER_ADDR: '127.0.0.1:' + gatewayServerPort, ASKBAR_GATEWAY_ADMIN_ADDR: '127.0.0.1:' + gatewayAdminPort,
    ASKBAR_GATEWAY_TOKEN_DIR: path.join(dirs.creds, 'model-gateway-token'), ASKBAR_AUDIT_API_URL: 'http://127.0.0.1:' + auditPort,
  });
  run(path.join(ROOT, 'target/debug/askbar-admin'), [], {
    ASKBAR_ADMIN_SOCKET: sock, ASKBAR_ADMIN_DATA_DIR: dirs.adminData, ASKBAR_ADMIN_SECRETS_DIR: dirs.adminSecrets,
    ASKBAR_ADMIN_TOKEN_DIR: path.join(dirs.creds, 'admin-token'), ASKBAR_SERVER_API_URL: 'http://127.0.0.1:' + serverApiPort,
    ASKBAR_GATEWAY_URL: 'http://127.0.0.1:' + gatewayAdminPort, ASKBAR_AUDIT_API_URL: 'http://127.0.0.1:' + auditPort,
    ASKBAR_ALERTER_URL: 'http://127.0.0.1:' + alerterAdminPort,
    ASKBAR_PUBLIC_URL: 'http://localhost:' + api,
    ASKBAR_BACKUP_DIR: path.join(data, 'backup'),
    ASKBAR_CPA_CONFIG_DIR: path.join(data, 'cpa-config'), ASKBAR_CPA_URL: 'http://127.0.0.1:' + await freePort(),
  });
  for (let i = 0; i < 100 && !fs.existsSync(sock); i++) await new Promise((r) => setTimeout(r, 50));
  fs.mkdirSync(path.join(data, 'backup'), { recursive: true });
  fs.mkdirSync(path.join(data, 'cpa-auth'), { recursive: true });
  fs.writeFileSync(path.join(data, 'cpa-auth', 'claude-e2e.json'), '{"access_token":"' + LOGIN_MARKER + '"}');
  run(supervisorBin, ['sleep', '100000'], { ASKBAR_CPA_CONFIG_DIR: path.join(data, 'cpa-config'), ASKBAR_CPA_AUTH_DIR: path.join(data, 'cpa-auth') });
  forwarder = net.createServer((c) => {
    const u = net.connect(sock);
    c.pipe(u); u.pipe(c);
    c.on('error', () => u.destroy()); u.on('error', () => c.destroy());
  }).listen(adminPort, '127.0.0.1');
  let serverProc = startServer();
  await waitForPort(adminPort);
  await waitForPort(api);
  await waitForPort(mockPort);

  const base = `http://127.0.0.1:${adminPort}/admin/`;
  browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport, permissions: ['clipboard-read', 'clipboard-write'] });
  const page = await ctx.newPage();
  const errors = [];
  // Each admin section is its own page (plan step 1.16): a nav link loads it, and its scripts run before
  // `load`, so wait for that before using the page.
  // On a phone the menu is behind the Menu button (plan step 2.4): open it before using what is in it.
  const openMenu = async () => {
    if (isPhone && (await page.getAttribute('#menu-open', 'aria-expanded')) !== 'true') await page.click('#menu-open');
  };
  const openPage = async (slug) => {
    await openMenu();
    await page.click('#nav [data-tab="' + slug + '"]');
    await page.waitForURL('**/admin/' + slug + '/**');
    await page.waitForLoadState('load');
    await page.waitForSelector('#app:not([hidden])');
  };
  // A closed side panel has also left its address (Back runs a moment after the panel hides).
  const panelClosed = async () => {
    await page.waitForSelector('#provider-panel', { state: 'hidden' });
    await page.waitForFunction(() => !location.search.includes('panel='));
  };
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of (4\d\d|502)/.test(m.text())) errors.push(m.text()); });

  // Opens the Settings tab and waits until its values have been read from the server. Waiting for a field to hold a
  // value is not enough when the test typed that same value: a read that lands later overwrites what the test types next.
  async function openSettings() {
    await openPage('providers');
    const reread = page.waitForResponse((r) => r.url().endsWith('/api/admin/v1/settings') && r.request().method() === 'GET');
    await openPage('general');
    await reread;
  }

  // The sign-in card (plan step 6.3): the first-run hint shows on the published login, the card has no attempts text,
  // Skip to content is the first Tab stop, the passkey row is greyed and inert, buttons are 40 px (44 on a phone),
  // nothing scrolls sideways, and axe is clean.
  await step('sign-in card: the first-run hint shows on the published login, no attempts text, Skip to content first, passkey greyed and inert, sizes and axe', async () => {
    const setupResponse = page.waitForResponse((r) => r.url().endsWith('/api/admin/v1/setup') && r.request().method() === 'GET');
    await page.goto(base);
    await page.waitForSelector('#login:not([hidden])');
    assert.equal((await setupResponse).status(), 200, 'the sign-in page reads the public setup status once');
    await page.waitForSelector('#login-hint:not([hidden])');
    assert.match(await page.innerText('#login-hint'), /published/);
    assert.equal(await page.evaluate(() => /attempt/i.test(document.getElementById('login').innerText)), false, 'no attempts text on the card');
    // The password field has focus on open (autofocus is expected here), so the Tab order is read from the document: the first
    // focusable element that is shown and in the tab sequence must be Skip to content.
    const firstStop = await page.evaluate(() => {
      const f = [...document.querySelectorAll('a[href], button, input, select, textarea, [tabindex]')].find((e) => e.tabIndex >= 0 && e.getClientRects().length > 0);
      return { cls: f.className, text: f.textContent };
    });
    assert.deepEqual(firstStop, { cls: 'skip-link', text: 'Skip to content' }, 'Skip to content is the first Tab stop');
    const passkey = await page.evaluate(() => {
      const row = document.querySelector('#login .na');
      return { ariaDisabled: row.getAttribute('aria-disabled'), tabindex: row.getAttribute('tabindex'), disabled: !!row.querySelector('[disabled]') || row.hasAttribute('disabled'), text: row.textContent.trim() };
    });
    assert.equal(passkey.ariaDisabled, 'true');
    assert.equal(passkey.tabindex, '0', 'reachable by Tab');
    assert.equal(passkey.disabled, false, 'no disabled attribute');
    assert.match(passkey.text, /^Use a passkey Not available yet: /);
    assert.equal(/\d/.test(passkey.text), false, 'no digits in the label');
    await page.focus('#login .na');
    await page.keyboard.press('Enter');
    assert.equal(await page.isHidden('#login'), false, 'Enter on the greyed passkey does nothing');
    const sizes = await page.evaluate(() => ({
      submit: document.querySelector('#login-form button[type=submit]').getBoundingClientRect().height,
      passkey: document.querySelector('#login .na .btn').getBoundingClientRect().height,
      scrolls: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    }));
    assert.ok(sizes.submit >= (isPhone ? 44 : 40) - 0.5 && sizes.submit <= (isPhone ? 44 : 40) + 0.5, 'Sign in is ' + sizes.submit + ' px high');
    assert.ok(sizes.passkey >= (isPhone ? 44 : 40) - 0.5, 'the passkey row is ' + sizes.passkey + ' px high');
    assert.equal(sizes.scrolls, false, 'no sideways scroll on the sign-in card');
    const axeCtx = await browser.newContext({ viewport, bypassCSP: true });
    try {
      const ap = await axeCtx.newPage();
      await ap.goto(base);
      await ap.waitForSelector('#login:not([hidden])');
      await ap.addScriptTag({ content: fs.readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8') });
      const found = await ap.evaluate(async () => (await window.axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } })).violations.map((v) => v.id + ': ' + v.nodes.map((n) => n.target.join(' ')).join(', ')));
      assert.deepEqual(found, [], 'axe on the sign-in card');
    } finally { await axeCtx.close(); }
  });

  await step('first run: sign in with the published login (ADR 0007)', async () => {
    await page.goto(base);
    await page.waitForSelector('#login:not([hidden])');
    assert.equal(await page.inputValue('#username'), 'admin');
    await page.fill('#username', 'root');
    await page.fill('#password', 'admin');
    await page.click('#login-form button');
    await page.waitForSelector('#password-error:not([hidden])');
    assert.match(await page.innerText('#password-error'), /Wrong username or password/, 'a wrong name gets the same answer as a wrong password');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'password', 'the failed sign-in moves focus to the password field');
    assert.equal(await page.getAttribute('#password', 'aria-invalid'), 'true');
    await page.fill('#username', 'admin');
    await page.fill('#password', 'admin');
    await page.click('#login-form button');
    await page.waitForSelector('#setup-password-form:not([hidden])');
    assert.equal(await page.isHidden('#app'), true, 'nothing else opens before the password is changed');
  });
  // First-run setup card (plan step 6.4): the three-step list, in the words and symbols the screens show.
  const setupSteps = () => page.evaluate(() => [...document.querySelectorAll('#setup-steps > li')].map((li) => ({ current: li.getAttribute('aria-current'), text: li.innerText.replace(/\s+/g, ' ').trim() })));
  const SETUP_NAMES = ['1. Change the password', '2. Audit passphrase', '3. Backup passphrase'];
  const stepWords = { done: '\u2713 Done', current: '\u25cf Current step', next: '\u25cb Next' };
  async function assertSetupSteps(states, label) {
    const got = await setupSteps();
    assert.deepEqual(got.map((x) => x.text), states.map((st, i) => stepWords[st] + ' ' + SETUP_NAMES[i]), label + ': the three steps, each a symbol, a word and its name');
    assert.deepEqual(got.map((x) => x.current), states.map((st) => (st === 'current' ? 'step' : null)), label + ': only the current step is aria-current');
  }
  // One setup screen: Skip to content is still the first Tab stop, the buttons and fields are 40 px (44 on a phone), nothing scrolls
  // sideways, and axe is clean on it (a second context with the same sign-in opens the same screen).
  async function checkSetupScreen(form, label) {
    const firstStop = await page.evaluate(() => {
      const f = [...document.querySelectorAll('a[href], button, input, select, textarea, [tabindex]')].find((e) => e.tabIndex >= 0 && e.getClientRects().length > 0);
      return { cls: f.className, text: f.textContent };
    });
    assert.deepEqual(firstStop, { cls: 'skip-link', text: 'Skip to content' }, label + ': Skip to content is the first Tab stop');
    const sizes = await page.evaluate((sel) => {
      const f = document.querySelector(sel);
      return {
        buttons: [...f.querySelectorAll('button:not([hidden])')].map((b) => b.getBoundingClientRect().height),
        inputs: [...f.querySelectorAll('input:not([type=checkbox])')].map((i) => i.getBoundingClientRect().height),
        checks: [...f.querySelectorAll('label.check')].map((l) => l.getBoundingClientRect().height),
        scrolls: document.documentElement.scrollWidth > document.documentElement.clientWidth,
      };
    }, form);
    const want = isPhone ? 44 : 40;
    for (const h of sizes.buttons) assert.ok(h >= want - 0.5, label + ': a button is ' + h + ' px high');
    for (const h of sizes.inputs) assert.ok(h >= want - 0.5, label + ': a field is ' + h + ' px high');
    if (isPhone) for (const h of sizes.checks) assert.ok(h >= 44 - 0.5, label + ': the check box row is ' + h + ' px high');
    assert.equal(sizes.scrolls, false, label + ': no sideways scroll');
    const axeCtx = await browser.newContext({ viewport, bypassCSP: true, storageState: await ctx.storageState() });
    try {
      const ap = await axeCtx.newPage();
      await ap.goto(base);
      await ap.waitForSelector(form + ':not([hidden])');
      await ap.addScriptTag({ content: fs.readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8') });
      const found = await ap.evaluate(async () => (await window.axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } })).violations.map((v) => v.id + ': ' + v.nodes.map((n) => n.target.join(' ')).join(', ')));
      assert.deepEqual(found, [], 'axe on ' + label);
    } finally { await axeCtx.close(); }
  }
  const AUDIT_PASS = 'correct horse battery staple 42';
const BACKUP_PASS = 'e2e backup passphrase, its own 1';
const BACKUP_PASS_2 = 'e2e backup passphrase, its own 2';
  await step('setup card: step 1 of 3 shows, the password has focus, counts are live, a mismatched pair blocks Next with a focused inline error and sends nothing, paste is allowed, sizes and axe', async () => {
    await assertSetupSteps(['current', 'next', 'next'], 'password screen');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'setup-current', 'the first field has focus on open');
    const { min } = await page.evaluate(async () => ({ min: (await window.ui.ready).MIN_PASSWORD_CHARS }));
    assert.equal(await page.innerText('#setup-password-hint'), 'At least ' + min + ' characters');
    await page.fill('#setup-password', 'abc');
    assert.equal(await page.innerText('#setup-password-hint'), '3 of at least ' + min + ' characters');
    await page.fill('#setup-password', 'x'.repeat(min));
    assert.equal(await page.innerText('#setup-password-hint'), min + ' characters');
    for (const id of ['setup-current', 'setup-password', 'setup-password2']) {
      assert.equal(await page.evaluate((i) => document.getElementById(i).dispatchEvent(new ClipboardEvent('paste', { cancelable: true, bubbles: true })), id), true, id + ' accepts a paste');
    }
    await checkSetupScreen('#setup-password-form', 'password screen');
    const sent = [];
    const watch = (r) => { if (r.method() !== 'GET') sent.push(r.method() + ' ' + r.url()); };
    page.on('request', watch);
    try {
      await page.fill('#setup-current', 'admin');
      await page.fill('#setup-password', PASSWORD);
      await page.fill('#setup-password2', PASSWORD + 'x');
      await page.click('#setup-password-form button[type=submit]');
      await page.waitForSelector('#setup-password2-error:not([hidden])');
      assert.match(await page.innerText('#setup-password2-error'), /do not match/);
      assert.equal(await page.getAttribute('#setup-password2', 'aria-invalid'), 'true');
      assert.equal(await page.evaluate(() => document.activeElement.id), 'setup-password2', 'focus moves to the repeat field');
      assert.match(await page.innerText('#setup-password-form .error-summary'), /Fix this to go on/);
      await page.waitForTimeout(300);
      assert.deepEqual(sent, [], 'no request was sent');
    } finally { page.off('request', watch); }
  });
  await step('setup: the published password must be changed, to something else, then the audit passphrase step opens', async () => {
    await page.fill('#setup-current', 'admin');
    await page.fill('#setup-password', PASSWORD);
    await page.fill('#setup-password2', PASSWORD + 'x');
    await page.click('#setup-password-form button[type=submit]');
    await page.waitForSelector('#setup-password2-error:not([hidden])');
    assert.match(await page.innerText('#setup-password2-error'), /do not match/);
    // The published password (`admin`) as the new one is refused (by the server too, not only the form).
    await page.evaluate(() => { document.getElementById('setup-password-form').noValidate = true; });
    await page.fill('#setup-password', 'admin');
    await page.fill('#setup-password2', 'admin');
    await page.click('#setup-password-form button[type=submit]');
    await page.waitForFunction(() => /at least 10/.test(document.getElementById('setup-password-error').textContent));
    assert.equal(await page.evaluate(() => document.activeElement.id), 'setup-password', 'focus goes to the short password');
    await page.fill('#setup-password', PASSWORD);
    await page.fill('#setup-password2', PASSWORD);
    await page.click('#setup-password-form button[type=submit]');
    await page.waitForSelector('#setup-audit-form:not([hidden])');
    assert.equal(await page.isHidden('#app'), true, 'setup does not finish at the password');
    assert.equal(await page.isHidden('#setup-password-form'), true);
    assert.equal(await page.getAttribute('#setup-audit', 'type'), 'password');
    assert.match(await page.innerText('#setup-audit-form'), /cannot be recovered/);
    // A new install can be set up from a backup instead: the link opens the Restore section of Backup without finishing the setup screens.
    await page.click('#setup-restore-link');
    await page.waitForURL('**/admin/backup/#restore-section');
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#restore-upload-form:not([hidden])');
    assert.equal(await page.isHidden('#setup'), true);
    await page.goto(base);
    await page.waitForSelector('#setup-audit-form:not([hidden])');
  });
  await step('setup card: step 2 of 3 shows on the audit screen, the counts are live, sizes and axe', async () => {
    await assertSetupSteps(['done', 'current', 'next'], 'audit screen');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'setup-audit', 'the passphrase has focus on open');
    const { min } = await page.evaluate(async () => ({ min: (await window.ui.ready).MIN_PASSPHRASE_CHARS }));
    assert.equal(await page.innerText('#setup-audit-hint'), 'At least ' + min + ' characters');
    await page.fill('#setup-audit', 'abc');
    assert.equal(await page.innerText('#setup-audit-hint'), '3 of at least ' + min + ' characters');
    await page.fill('#setup-audit', '');
    await checkSetupScreen('#setup-audit-form', 'audit screen');
  });
  await step('setup: the audit passphrase must match, be 20+ characters and be confirmed as stored', async () => {
    await page.evaluate(() => document.getElementById('setup-audit-form').noValidate = true);
    await page.fill('#setup-audit', AUDIT_PASS);
    await page.fill('#setup-audit2', AUDIT_PASS + 'x');
    await page.check('#setup-audit-stored');
    await page.click('#setup-audit-form button[type=submit]');
    assert.match(await page.innerText('#setup-audit2-error'), /do not match/);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'setup-audit2', 'the mismatch is on the repeat field, focused');
    await page.fill('#setup-audit', 'too short');
    await page.fill('#setup-audit2', 'too short');
    await page.click('#setup-audit-form button[type=submit]');
    assert.match(await page.innerText('#setup-audit-error'), /at least 20/);
    // Without the confirmation nothing is created, even with a good passphrase.
    await page.fill('#setup-audit', AUDIT_PASS);
    await page.fill('#setup-audit2', AUDIT_PASS);
    await page.uncheck('#setup-audit-stored');
    await page.click('#setup-audit-form button[type=submit]');
    assert.match(await page.innerText('#setup-audit-stored-error'), /stored the passphrase/);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'setup-audit-stored', 'the unchecked box is focused');
    assert.equal(await page.isHidden('#app'), true);
    const st = await (await fetch(`http://127.0.0.1:${adminPort}/api/admin/v1/setup`, { headers: { 'x-askbar-admin': '1' } })).json();
    assert.equal(st.audit_key_set, false, 'no key yet');
  });
  await step('setup: saving shows the key id, then Continue goes on to the backup passphrase', async () => {
    await page.check('#setup-audit-stored');
    await page.click('#setup-audit-form button[type=submit]');
    await page.waitForSelector('#setup-audit-done:not([hidden])');
    await assertSetupSteps(['done', 'done', 'next'], 'audit key ready');
    const keyId = await page.innerText('#setup-audit-keyid');
    assert.match(keyId, /^[0-9a-f]{16}$/);
    assert.equal(await page.inputValue('#setup-audit').catch(() => ''), '', 'the passphrase field is cleared');
    assert.equal((await page.content()).includes(AUDIT_PASS), false, 'the passphrase is not left in the page');
    await page.click('#setup-audit-continue');
    await page.waitForSelector('#setup-backup-form:not([hidden])');
    await assertSetupSteps(['done', 'done', 'current'], 'backup screen');
    assert.equal(await page.isHidden('#app'), true, 'setup does not finish at the audit key');
    assert.equal(await page.getAttribute('#setup-backup', 'type'), 'password');
    assert.match(await page.innerText('#setup-backup-form'), /cannot be recovered/);
  });
  await step('setup card: step 3 of 3 shows on the backup screen, the counts are live, sizes and axe', async () => {
    assert.equal(await page.evaluate(() => document.activeElement.id), 'setup-backup', 'the passphrase has focus on open');
    const { min } = await page.evaluate(async () => ({ min: (await window.ui.ready).MIN_PASSPHRASE_CHARS }));
    await page.fill('#setup-backup', 'abc');
    assert.equal(await page.innerText('#setup-backup-hint'), '3 of at least ' + min + ' characters');
    await page.fill('#setup-backup', '');
    await checkSetupScreen('#setup-backup-form', 'backup screen');
  });
  await step('setup: the backup passphrase must match, be 20+ characters, differ from the audit passphrase and be confirmed as stored (ADR 0013)', async () => {
    await page.evaluate(() => document.getElementById('setup-backup-form').noValidate = true);
    await page.fill('#setup-backup', BACKUP_PASS);
    await page.fill('#setup-backup2', BACKUP_PASS + 'x');
    await page.check('#setup-backup-stored');
    await page.click('#setup-backup-form button[type=submit]');
    assert.match(await page.innerText('#setup-backup2-error'), /do not match/);
    await page.fill('#setup-backup', 'too short');
    await page.fill('#setup-backup2', 'too short');
    await page.click('#setup-backup-form button[type=submit]');
    assert.match(await page.innerText('#setup-backup-error'), /at least 20/);
    // The audit passphrase is refused by the admin, which knows it can open an audit key.
    await page.fill('#setup-backup', AUDIT_PASS);
    await page.fill('#setup-backup2', AUDIT_PASS);
    await page.click('#setup-backup-form button[type=submit]');
    await page.waitForFunction(() => /different from the audit passphrase/.test(document.getElementById('setup-backup-form-error').textContent));
    // Without the confirmation nothing is created, even with a good passphrase.
    await page.fill('#setup-backup', BACKUP_PASS);
    await page.fill('#setup-backup2', BACKUP_PASS);
    await page.uncheck('#setup-backup-stored');
    await page.click('#setup-backup-form button[type=submit]');
    assert.match(await page.innerText('#setup-backup-stored-error'), /stored the passphrase/);
    assert.equal(await page.isHidden('#app'), true);
    const st = await (await fetch(`http://127.0.0.1:${adminPort}/api/admin/v1/setup`, { headers: { 'x-askbar-admin': '1' } })).json();
    assert.equal(st.backup_key_set, false, 'no backup key yet');
  });
  await step('setup: saving shows the fingerprint and the next steps, then Continue opens the app', async () => {
    await page.check('#setup-backup-stored');
    await page.click('#setup-backup-form button[type=submit]');
    await page.waitForSelector('#setup-backup-done:not([hidden])');
    await assertSetupSteps(['done', 'done', 'done'], 'backup key ready');
    assert.match(await page.innerText('#setup-backup-fingerprint'), /^[0-9a-f]{4}(-[0-9a-f]{4}){3}$/);
    assert.equal(await page.inputValue('#setup-backup').catch(() => ''), '', 'the passphrase field is cleared');
    assert.equal(await page.inputValue('#setup-backup2').catch(() => ''), '');
    assert.equal((await page.content()).includes(BACKUP_PASS), false, 'the passphrase is not left in the page');
    assert.match(await page.innerText('#setup-next'), /Network[\s\S]*Providers[\s\S]*People & clients/, 'the next steps, in order');
    await page.click('#setup-backup-continue');
    await page.waitForSelector('#app:not([hidden])');
    assert.equal(await page.isHidden('#setup'), true);
    // The app opens on Home (the landing page, plan step 4.1); the providers are one click away.
    await openPage('providers');
    assert.match(await page.innerText('#providers'), /No providers yet/);
    assert.equal(await page.locator('#setup-key, #setup-saved').count(), 0, 'the master key screens are gone');
  });
  await step('the server makes no master key (it holds no provider key), and the admin page shows no key', async () => {
    assert.equal(fs.existsSync(path.join(dirs.serverSecrets, 'master.key')), false, 'the server has no master key (ADR 0054)');
    await page.reload();
    await page.waitForSelector('#app:not([hidden])');
    assert.equal(await page.isHidden('#setup'), true);
  });
  await step('sign out, then a wrong password is refused and the right one signs in', async () => {
    await openMenu();
    const setupRead = page.waitForResponse((r) => r.url().endsWith('/api/admin/v1/setup') && r.request().method() === 'GET');
    await page.click('#logout');
    await page.waitForSelector('#login:not([hidden])');
    await setupRead;
    await page.waitForTimeout(300); // the response is read a moment after it arrives
    assert.equal(await page.isHidden('#login-hint'), true, 'the published password is changed: no first-run hint');
    await page.fill('#password', 'nope');
    await page.click('#login-form button');
    await page.waitForSelector('#password-error:not([hidden])');
    assert.match(await page.innerText('#password-error'), /Wrong username or password/);
    assert.match(await page.innerText('#login-form .error-summary'), /Sign-in did not work/);
    await page.fill('#password', PASSWORD);
    await page.click('#login-form button');
    await page.waitForSelector('#app:not([hidden])');
  });

  await step('add the mock provider through the page: the panel saves it, then tests it', async () => {
    await page.click('#add-provider');
    await page.waitForSelector('#provider-panel:not([hidden])');
    assert.equal(await page.innerText('#provider-submit'), 'Add and test');
    assert.match(page.url(), /[?&]panel=provider-panel/, 'the panel has its own address');
    await page.fill('#p-name', 'mock');
    await page.fill('#p-base-url', `http://127.0.0.1:${mockPort}/v1`);
    await page.fill('#p-model', 'mock-model');
    await page.fill('#p-key', SECRET_KEY);
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('tr[data-name="mock"]');
    assert.match(await page.innerText('tr[data-name="mock"]'), /key set/);
    await panelClosed();
    // The test runs after the save, with no click on Test.
    await page.waitForFunction(() => /OK/.test(document.querySelector('tr[data-name="mock"] .test-result').textContent), null, { timeout: 15000 });
    assert.match(await page.innerText('tr[data-name="mock"] .test-result'), /Streaming works/);
  });
  // Buttons (rules 18-20): the default is 32 px, .btn-s 28 px and .btn-l 40 px. The probes are built with createElement.
  // On a phone (390 wide) every button is at least --btn-h-touch (44 px) high.
  await step(isPhone ? 'buttons: a default, .btn-s and .btn-l button are each 44 px high on a phone' : 'buttons: a default button is 32 px high, .btn-s 28 px and .btn-l 40 px', async () => {
    const heights = await page.evaluate(() => {
      const probe = (cls) => {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = cls;
        b.textContent = 'Probe';
        document.body.appendChild(b);
        const h = getComputedStyle(b).height;
        b.remove();
        return h;
      };
      return {
        existing: getComputedStyle(document.querySelector('#add-provider')).height,
        plain: probe('btn'),
        small: probe('btn btn-s'),
        large: probe('btn btn-l'),
      };
    });
    if (isPhone) {
      for (const [k, h] of Object.entries(heights)) assert.ok(parseFloat(h) >= 44, k + ' ' + JSON.stringify(heights));
    } else {
      assert.deepEqual(heights, { existing: '32px', plain: '32px', small: '28px', large: '40px' }, JSON.stringify(heights));
    }
  });
  // Type scale (rules 14, 15, 23): the title is 1.3rem/600, a field label 0.875rem/600, help 0.75rem/400, body 0.875rem/400.
  await step('type scale: title 20.8px/600, label 14px/600, help 12px/400, body 14px/400', async () => {
    const type = await page.evaluate(() => {
      const read = (el) => { const s = getComputedStyle(el); return { size: parseFloat(s.fontSize), weight: s.fontWeight }; };
      return {
        title: read(document.querySelector('section[data-tab] > .row > h2')),
        label: read(document.querySelector('#provider-form label:not(.check)')),
        help: read(document.querySelector('section[data-tab] > p.hint')),
        body: read(document.body),
      };
    });
    const near = (a, b) => Math.abs(a - b) <= 0.1;
    assert.ok(near(type.title.size, 20.8) && type.title.weight === '600', JSON.stringify(type));
    assert.ok(near(type.label.size, 14) && type.label.weight === '600', JSON.stringify(type));
    assert.ok(near(type.help.size, 12) && type.help.weight === '400', JSON.stringify(type));
    assert.ok(near(type.body.size, 14) && type.body.weight === '400', JSON.stringify(type));
  });
  // The component builders (plan step 1.9). No page uses them yet, so each step builds the server-rendered half of a
  // component with createElement (the classes and data-* attributes the macros in components.html write) and drives
  // it through window.ui, then removes it.
  await step('components: a side panel docks with its own address; Esc asks before dropping typed input; Back and × close it', async () => {
    await page.evaluate(() => {
      const { el } = window.ui;
      const opener = el('button', { type: 'button', id: 't-opener', class: 'btn', text: 'Open', onclick: () => window.ui.openPanel('t-panel') });
      const panel = el('aside', { id: 't-panel', class: 'side-panel', hidden: true },
        el('header', { class: 'side-panel-head' }, el('h2', { class: 'side-panel-title', text: 'Test panel' }),
          el('button', { type: 'button', class: 'btn btn-quiet btn-icon', 'aria-label': 'Close', 'data-panel-close': true })),
        el('div', { class: 'side-panel-body' }, el('label', { for: 't-in', text: 'Name' }), el('input', { id: 't-in', type: 'text' })));
      document.querySelector('#app main').append(opener, panel);
    });
    const start = page.url();
    await page.click('#t-opener');
    await page.waitForSelector('#t-panel:not([hidden])');
    assert.match(page.url(), /[?&]panel=t-panel/, 'the panel has its own address');
    assert.equal(await page.evaluate(() => document.body.classList.contains('panel-open')), true);
    if (!isPhone) assert.equal(await page.evaluate(() => getComputedStyle(document.getElementById('app')).marginRight), '440px', 'the page narrows, nothing covers it');
    await page.fill('#t-in', 'typed');
    await page.keyboard.press('Escape');
    await page.waitForSelector('dialog[open] >> text=Discard your changes?');
    await page.click('dialog[open] [data-confirm-cancel]');
    assert.equal(await page.isVisible('#t-panel'), true, 'Keep editing keeps the panel');
    assert.equal(await page.inputValue('#t-in'), 'typed');
    await page.focus('#t-in');
    await page.keyboard.press('Escape');
    await page.click('dialog[open] [data-confirm-action]');
    await page.waitForSelector('#t-panel', { state: 'hidden' });
    assert.equal(page.url(), start, 'closing leaves the panel\'s address');
    assert.equal(await page.evaluate(() => document.activeElement.id), 't-opener', 'focus returns to the opener');

    await page.click('#t-opener');
    await page.waitForSelector('#t-panel:not([hidden])');
    await page.goBack();
    await page.waitForSelector('#t-panel', { state: 'hidden' });
    assert.equal(page.url(), start, 'Back closes the panel and stays on the page');
    await page.click('#t-opener');
    await page.waitForSelector('#t-panel:not([hidden])');
    await page.click('#t-panel [data-panel-close]');
    await page.waitForSelector('#t-panel', { state: 'hidden' });
    assert.equal(page.url(), start);
    assert.equal(await page.evaluate(() => document.body.classList.contains('panel-open')), false);
    await page.evaluate(() => { document.getElementById('t-opener').remove(); document.getElementById('t-panel').remove(); });
  });
  await step('components: a menu opens on its first item, moves with the arrow keys, Home and End, and Esc returns to its button', async () => {
    await page.evaluate(() => {
      const { el } = window.ui;
      window.tPicked = [];
      const button = el('button', { type: 'button', id: 't-menu', class: 'btn', text: 'More' });
      document.querySelector('#app main').append(el('div', { id: 't-menu-wrap' }, button));
      window.ui.menu(button, [
        { label: 'Edit', action: () => window.tPicked.push('edit') },
        { label: 'Delete', danger: true, action: () => window.tPicked.push('delete') },
      ]);
    });
    const focused = () => page.evaluate(() => document.activeElement.textContent);
    await page.focus('#t-menu');
    await page.keyboard.press('Enter');
    assert.equal(await page.getAttribute('#t-menu', 'aria-expanded'), 'true');
    assert.equal(await focused(), 'Edit');
    await page.keyboard.press('ArrowDown');
    assert.equal(await focused(), 'Delete');
    await page.keyboard.press('ArrowDown');
    assert.equal(await focused(), 'Edit', 'the arrows wrap');
    await page.keyboard.press('End');
    assert.equal(await focused(), 'Delete');
    await page.keyboard.press('Home');
    assert.equal(await focused(), 'Edit');
    await page.keyboard.press('Escape');
    assert.equal(await page.getAttribute('#t-menu', 'aria-expanded'), 'false');
    assert.equal(await page.evaluate(() => document.activeElement.id), 't-menu', 'Esc returns focus to the button');
    await page.keyboard.press('ArrowUp');
    assert.equal(await focused(), 'Delete', 'ArrowUp opens on the last item');
    await page.keyboard.press('Enter');
    assert.deepEqual(await page.evaluate(() => window.tPicked), ['delete']);
    assert.equal(await page.isHidden('#t-menu-wrap .menu'), true, 'choosing closes the menu');
    await page.evaluate(() => document.getElementById('t-menu-wrap').remove());
  });
  await step('components: the length hint counts as you type, a typed-name confirm waits for the name, and a toast leaves after TOAST_SECS', async () => {
    const min = await page.evaluate(async () => (await window.ui.ready).MIN_PASSPHRASE_CHARS);
    await page.evaluate((m) => {
      const { el } = window.ui;
      const input = el('input', { id: 't-pass', type: 'password' });
      document.querySelector('#app main').append(el('div', { id: 't-len', class: 'field' }, input,
        el('p', { class: 'hint', id: 't-pass-hint', 'data-length-hint': true, text: 'At least ' + m + ' characters' })));
      window.ui.lengthHint(input, m);
    }, min);
    await page.fill('#t-pass', 'abc');
    assert.equal(await page.innerText('#t-pass-hint'), '3 of at least ' + min + ' characters');
    await page.fill('#t-pass', 'x'.repeat(min));
    assert.equal(await page.innerText('#t-pass-hint'), min + ' characters');
    await page.evaluate(() => document.getElementById('t-len').remove());

    await page.evaluate(() => { window.tConfirm = window.ui.confirm({ title: 'Delete mock?', body: 'It cannot be undone.', action: 'Delete', typeName: 'mock' }); });
    await page.waitForSelector('dialog[open] >> text=Delete mock?');
    assert.equal(await page.isDisabled('dialog[open] [data-confirm-action]'), true, 'the action waits for the name');
    await page.fill('dialog[open] input', 'moc');
    assert.equal(await page.isDisabled('dialog[open] [data-confirm-action]'), true);
    await page.fill('dialog[open] input', 'mock');
    await page.click('dialog[open] [data-confirm-action]');
    assert.equal(await page.evaluate(() => window.tConfirm), true);
    assert.equal(await page.locator('dialog:not(#unseal-box)').count(), 0, 'the box is removed once answered');

    const toastMs = await page.evaluate(async () => (await window.ui.ready).TOAST_SECS * window.ui.MS);
    await page.evaluate(() => window.ui.toast('Saved the test'));
    assert.equal(await page.innerText('#toasts'), 'Saved the test');
    await page.waitForFunction(() => document.getElementById('announcer').textContent === 'Saved the test');
    await page.waitForTimeout(toastMs - 1000);
    assert.equal(await page.locator('#toasts .toast').count(), 1, 'still there before TOAST_SECS');
    await page.waitForSelector('#toasts .toast', { state: 'detached', timeout: 3000 });
  });
  await step('components: field errors, row confirmation, time, numbers and copy', async () => {
    const r = await page.evaluate(async () => {
      const { el } = window.ui;
      const out = {};
      document.querySelector('#app main').append(el('div', { id: 't-misc' },
        el('input', { id: 't-f', type: 'text' }), el('p', { id: 't-f-error', hidden: true }),
        el('div', { id: 't-row' }, el('button', { type: 'button', id: 't-restart', text: 'Restart' })),
        el('code', { id: 't-secret', text: 'askbar-admin-xyz' }), el('button', { type: 'button', id: 't-copy', class: 'btn btn-icon', 'aria-label': 'Copy', 'data-copy': 't-secret' })));
      window.ui.fieldError('t-f', 'Not a host name.');
      out.invalid = document.getElementById('t-f').getAttribute('aria-invalid');
      out.error = document.getElementById('t-f-error').hidden ? null : document.getElementById('t-f-error').textContent;
      window.ui.fieldError('t-f', '');
      out.cleared = document.getElementById('t-f').hasAttribute('aria-invalid') || !document.getElementById('t-f-error').hidden;
      const t = window.ui.time(Date.now() / 1000 - 3 * 3600);
      out.time = [t.tagName, t.textContent, !!t.title, t.getAttribute('datetime').endsWith('Z')];
      out.num = window.ui.num(12345, 'tokens').textContent;
      out.status = window.ui.status('ok', 'Running').textContent;
      return out;
    });
    assert.equal(r.invalid, 'true');
    assert.equal(r.error, 'Not a host name.');
    assert.equal(r.cleared, false);
    assert.deepEqual(r.time, ['TIME', '3 hours ago', true, true]);
    assert.equal(r.num, '12,345 tokens');
    assert.equal(r.status, 'Running');

    await page.evaluate(() => {
      window.tRestarted = 0;
      const b = document.getElementById('t-restart');
      b.addEventListener('click', () => window.ui.rowConfirm(document.getElementById('t-row'), {
        text: 'Restart the server?', action: 'Restart', trigger: b, onConfirm: async () => { window.tRestarted++; },
      }));
    });
    await page.click('#t-restart');
    assert.equal(await page.isHidden('#t-restart'), true, 'the row\'s own button hides while it asks');
    assert.equal(await page.innerText('#t-row .row-confirm-text'), 'Restart the server?');
    await page.click('#t-row [data-row-cancel]');
    assert.equal(await page.isVisible('#t-restart'), true, 'Cancel brings the button back');
    assert.equal(await page.locator('#t-row .row-confirm').count(), 0);
    await page.click('#t-restart');
    await page.click('#t-row [data-row-action]');
    await page.waitForFunction(() => window.tRestarted === 1);
    assert.equal(await page.isVisible('#t-restart'), true);

    // ui.rows holds a row with an open confirmation, and keeps its confirmation row right after it, on a refresh, a
    // forced refresh and a reorder; both go when the row goes.
    const r2 = await page.evaluate(() => {
      const { el, rows, rowConfirm } = window.ui;
      const tbody = el('tbody', { id: 't-body' });
      document.getElementById('t-misc').append(el('table', null, tbody));
      const draw = (names, opts = {}) => rows(tbody, names.map((n) => ({ id: n })), (i) => el('tr', null, el('td', null, el('button', { type: 'button', text: i.id }))), opts);
      draw(['a', 'b', 'c']);
      const b = tbody.querySelector('[data-key="b"]');
      const trig = b.querySelector('button');
      rowConfirm(b, { text: 'Restart b?', action: 'Restart', trigger: trig, onConfirm: async () => {} });
      const order = () => [...tbody.children].map((x) => x.dataset.key || (x.classList.contains('row-confirm-row') ? '?' : '-')).join('');
      const out = { before: order() };
      draw(['a', 'b', 'c']);
      out.refresh = order();
      draw(['a', 'b', 'c'], { force: true });
      out.force = order();
      out.sameNode = tbody.querySelector('[data-key="b"]') === b;
      draw(['c', 'b', 'a']);
      out.reorder = order();
      out.hidden = trig.hidden;
      draw(['a', 'c']);
      out.gone = order();
      return out;
    });
    assert.deepEqual(r2, { before: 'ab?c', refresh: 'ab?c', force: 'ab?c', sameNode: true, reorder: 'cb?a', hidden: true, gone: 'ac' });
    await page.click('#t-copy');
    await page.waitForFunction(() => document.getElementById('t-copy').getAttribute('aria-label') === 'Copied');
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'askbar-admin-xyz');
    await page.evaluate(() => document.getElementById('t-misc').remove());
  });
  await step('approved models: none at first; tick two in the picker, choose the default, the save bar appears labelled Save, and it persists', async () => {
    assert.match(await page.innerText('#catalog-approved'), /No model is approved yet/);
    assert.equal(await page.isHidden('#catalog-form-save-bar'), true, 'nothing changed, so no save bar');
    assert.match(await page.innerText('#catalog-offered'), /Not loaded/, 'nothing is asked of a provider until Reload');
    await page.click('#catalog-load');
    await page.waitForSelector('#catalog-offered input[type=checkbox]');
    assert.deepEqual(await page.$$eval('#catalog-offered code', (c) => c.map((x) => x.textContent.trim())), ['mock-model', 'mock-model-2']);
    await page.click('#catalog-offered input[type=checkbox] >> nth=0');
    await page.waitForSelector('#catalog-form-save-bar:not([hidden])');
    assert.equal(await page.innerText('#catalog-form-save-bar [data-save-submit]'), 'Save');
    assert.match(await page.innerText('#catalog-form-save-bar [data-save-text]'), /Unsaved changes: 1 added/);
    const defaultModel = () => page.$eval('#catalog-approved .is-default', (r) => r.dataset.model);
    assert.match(await defaultModel(), /\/mock-model$/, 'the first one approved becomes the default');
    await page.click('#catalog-offered input[type=checkbox] >> nth=1');
    await page.click('#catalog-approved [data-model$="/mock-model-2"] input[type=radio]');
    assert.match(await defaultModel(), /\/mock-model-2$/);
    assert.match(await page.innerText('#catalog-approved-title'), /· 2$/);
    await page.click('#catalog-form-save-bar [data-save-submit]');
    await page.waitForFunction(() => document.getElementById('catalog-form-save-bar').hidden);
    await page.reload();
    await page.waitForSelector('#catalog-approved [data-model]');
    assert.equal(await page.locator('#catalog-approved [data-model]').count(), 2);
    assert.match(await defaultModel(), /\/mock-model-2$/, 'the default persisted');
    assert.equal(await page.isHidden('#catalog-form-save-bar'), true);
    // Removing the default leaves the next one as the default, never none.
    await page.click('#catalog-approved [data-model$="/mock-model-2"] button');
    assert.match(await defaultModel(), /\/mock-model$/);
    await page.waitForSelector('#catalog-form-save-bar:not([hidden])');
    await page.click('#catalog-form-save-bar [data-save-discard]');
    await page.waitForFunction(() => document.querySelectorAll('#catalog-approved [data-model]').length === 2);
    assert.match(await defaultModel(), /\/mock-model-2$/, 'discarding goes back to what is saved');
    assert.equal(await page.isHidden('#catalog-form-save-bar'), true);
  });
  await step('Test sends a message through the provider and shows success', async () => {
    await page.click('tr[data-name="mock"] [data-action="test"]');
    await page.waitForFunction(() => /OK|Failed/.test(document.querySelector('tr[data-name="mock"] .test-result').textContent));
    assert.match(await page.innerText('tr[data-name="mock"] .test-result'), /^OK\s+Streaming works\. The first text came after \d+ ms\./);
    assert.equal(await page.locator('tr[data-name="mock"] .test-result .status-ok').count(), 1, 'a word and a symbol, not colour alone');
    // The details are folded away until asked for, and are text: the request, the status and the timings, nothing else.
    assert.equal(await page.isVisible('tr[data-name="mock"] .test-details dl'), false);
    await page.click('tr[data-name="mock"] .test-details summary');
    const details = await page.innerText('tr[data-name="mock"] .test-details dl');
    assert.match(details, /Request\s+POST \/v1\/chat\/completions/);
    assert.match(details, /Status\s+200/);
    assert.match(details, /Time to first text\s+\d+ ms/);
    assert.equal(details.includes(SECRET_KEY), false);
  });
  await step('after a reload the key is not displayed anywhere', async () => {
    await page.reload();
    await page.waitForSelector('tr[data-name="mock"]');
    assert.equal((await page.content()).includes(SECRET_KEY), false);
    await page.click('tr[data-name="mock"] [data-action="edit"]');
    await page.waitForSelector('#provider-panel:not([hidden])');
    assert.equal(await page.inputValue('#p-key'), '');
    assert.match(await page.innerText('#p-key-hint'), /Empty keeps the current key/);
    assert.equal(await page.getAttribute('#p-key', 'type'), 'password');
    assert.equal(await page.innerText('#provider-submit'), 'Save');
    const values = await page.$$eval('#provider-form input, #provider-form select', (els) => els.map((e) => e.value).join('|'));
    assert.equal(values.includes(SECRET_KEY), false);
  });
  await step('editing a provider with a saved key offers the models it lists under the test model field', async () => {
    assert.equal(await page.locator('#p-model-options option').count(), 0, 'nothing is listed until Load models is pressed');
    await page.click('#p-model-refresh');
    await page.waitForFunction(() => document.querySelectorAll('#p-model-options option').length === 2);
    assert.deepEqual(await page.$$eval('#p-model-options option', (o) => o.map((x) => x.value)), ['mock-model', 'mock-model-2']);
    assert.equal(await page.inputValue('#p-model'), 'mock-model', 'the saved model is filled in');
    assert.match(await page.innerText('#p-model-hint'), /2 models available/);
    assert.equal(await page.isHidden('#p-key'), false);
    assert.equal(await page.inputValue('#p-key'), '', 'the saved key was used by the server; the page never held it');
  });
  await step('a model name can be typed even when the provider lists some', async () => {
    await page.fill('#p-model', 'a-name-i-typed');
    assert.equal(await page.inputValue('#p-model'), 'a-name-i-typed');
    assert.equal(await page.locator('#p-model-options option').count(), 2, 'the list is only a help');
  });
  await step('editing without a new key keeps the old one working', async () => {
    await page.fill('#p-model', 'mock-model-2');
    await page.click('#provider-form button[type=submit]');
    await panelClosed();
    await page.click('tr[data-name="mock"] [data-action="edit"]');
    await page.waitForSelector('#provider-panel:not([hidden])');
    assert.equal(await page.inputValue('#p-model'), 'mock-model-2', 'the change was saved');
    await page.click('#provider-cancel');
    await panelClosed();
    await page.click('tr[data-name="mock"] [data-action="test"]');
    await page.waitForFunction(() => /OK|Failed/.test(document.querySelector('tr[data-name="mock"] .test-result').textContent));
    assert.match(await page.innerText('tr[data-name="mock"] .test-result'), /^OK/);
  });
  await step('provider billing: set a subscription with prices, see it kept, refuse a negative price', async () => {
    await page.click('tr[data-name="mock"] [data-action="edit"]');
    await page.waitForSelector('#provider-panel:not([hidden])');
    assert.equal(await page.inputValue('#p-billing'), 'metered', 'the default');
    assert.equal(await page.locator('#p-fallback').count(), 0, 'there is no provider fallback (ADR 0014)');
    await page.selectOption('#p-billing', 'subscription');
    await page.fill('#p-price-in', '2.5');
    await page.fill('#p-price-out', '10');
    await page.fill('#p-price-cached', '0.25');
    await page.click('#provider-form button[type=submit]');
    await panelClosed();
    // It is kept: edit again, the values are back.
    await page.click('tr[data-name="mock"] [data-action="edit"]');
    await page.waitForSelector('#provider-panel:not([hidden])');
    assert.equal(await page.inputValue('#p-billing'), 'subscription');
    assert.equal(await page.inputValue('#p-price-in'), '2.5');
    assert.equal(await page.inputValue('#p-price-out'), '10');
    assert.equal(await page.inputValue('#p-price-cached'), '0.25');
    // A negative price is refused with the server's rule, and nothing changes.
    await page.evaluate(() => document.getElementById('provider-form').noValidate = true);
    await page.fill('#p-price-in', '-1');
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('#provider-error:not([hidden])');
    assert.match(await page.innerText('#provider-error'), /zero or more/);
    assert.equal(await page.isVisible('#provider-panel'), true, 'the panel stays open with what was typed');
    // Put it back to an ordinary metered provider for the rest of the run.
    await page.fill('#p-price-in', '');
    await page.fill('#p-price-out', '');
    await page.fill('#p-price-cached', '');
    await page.selectOption('#p-billing', 'metered');
    await page.click('#provider-form button[type=submit]');
    await panelClosed();
    await page.click('tr[data-name="mock"] [data-action="edit"]');
    await page.waitForSelector('#provider-panel:not([hidden])');
    assert.equal(await page.inputValue('#p-billing'), 'metered');
    assert.equal(await page.inputValue('#p-price-in'), '');
    await page.click('#provider-cancel');
    await panelClosed();
  });
  await step('a provider that cannot be reached shows its error, from the test that follows the add', async () => {
    const dead = await freePort();
    await page.click('#add-provider');
    await page.waitForSelector('#provider-panel:not([hidden])');
    await page.fill('#p-name', 'dead');
    await page.fill('#p-base-url', `http://127.0.0.1:${dead}/v1`);
    await page.fill('#p-model', 'm');
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('tr[data-name="dead"]');
    await panelClosed();
    await page.waitForFunction(() => /Failed/.test(document.querySelector('tr[data-name="dead"] .test-result').textContent), null, { timeout: 15000 });
    assert.match(await page.innerText('tr[data-name="dead"] .test-result'), /Could not reach the provider.s address/);
    assert.equal(await page.locator('tr[data-name="dead"] .test-result .status-error').count(), 1);
    // Its models cannot be listed either: after Reload the picker says so next to a way to try again.
    await page.click('#catalog-load');
    await page.waitForSelector('#catalog-offered .picker-fail');
    assert.match(await page.innerText('#catalog-offered .picker-fail'), /Could not load models/);
    assert.equal(await page.locator('#catalog-offered .picker-fail button').count(), 1);
  });
  await step('a duplicate name is explained, and Esc or Cancel with typed input asks before dropping it', async () => {
    await page.click('#add-provider');
    await page.waitForSelector('#provider-panel:not([hidden])');
    await page.fill('#p-name', 'dead');
    await page.fill('#p-model', 'm');
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('#provider-error:not([hidden])');
    assert.match(await page.innerText('#provider-error'), /already exists/);
    await page.keyboard.press('Escape');
    await page.waitForSelector('dialog[open] >> text=Discard your changes?');
    await page.click('dialog[open] [data-confirm-cancel]');
    assert.equal(await page.isVisible('#provider-panel'), true, 'Keep editing keeps the panel and what was typed');
    assert.equal(await page.inputValue('#p-name'), 'dead');
    await page.click('#provider-cancel');
    await page.waitForSelector('dialog[open] >> text=Discard your changes?');
    await page.click('dialog[open] [data-confirm-action]');
    await panelClosed();
  });
  await step('a refusal reaches the page as a problem: a stable code and a sentence', async () => {
    const r = await page.evaluate(async () => {
      const res = await fetch('/api/admin/v1/providers/no-such-provider/test', { method: 'POST', headers: { 'X-Askbar-Admin': '1' } });
      return { status: res.status, type: res.headers.get('content-type'), body: await res.json() };
    });
    assert.equal(r.status, 404);
    assert.equal(r.type, 'application/problem+json');
    assert.equal(r.body.error, 'not_found');
    assert.ok(r.body.detail && r.body.title && r.body.type === 'urn:askbar:problem:not_found', JSON.stringify(r.body));
  });
  await step('CLIProxyAPI: the accounts panel is on the Providers page, and says so when CLIProxyAPI is not answering', async () => {
    await openPage('providers');
    await page.waitForSelector('#cpa-panel:not([hidden])');
    await page.waitForFunction(() => /not answering/.test(document.getElementById('cpa-message').textContent));
    await page.click('#cpa-connect-anthropic');
    await page.waitForFunction(() => /CLIProxyAPI did not answer/.test(document.getElementById('cpa-message').textContent));
    assert.equal(await page.isHidden('#cpa-login'), true, 'no login opens when CLIProxyAPI cannot start one');
    assert.equal(await page.locator('#add-cpa').count(), 0, 'the old preset button is gone');
  });
  // The paste step is the address the browser lands on (http://localhost...). With no domain it is always needed; with a
  // domain the admin waits for CLIProxyAPI to confirm, and the step shows only if the login did not finish.
  await step('CLIProxyAPI sign-in: the localhost paste step shows with no domain, and with a domain only when the login did not finish', async () => {
    const pollSecs = await page.evaluate(async () => (await window.ui.ready).CPA_LOGIN_POLL_SECS);
    let hostname = '';
    let loginStatus = { status: 'pending' };
    const json = (route, body) => route.fulfill({ contentType: 'application/json', body: JSON.stringify(body) });
    await page.route('**/api/admin/v1/network', (route) => (route.request().method() === 'GET' ? json(route, { hostname }) : route.continue()));
    await page.route('**/api/admin/v1/cpa', (route) => json(route, { configured: true, reachable: true, accounts: [] }));
    await page.route('**/api/admin/v1/cpa/logins', (route) => json(route, { url: 'https://login.example.test/authorize', state: 'st1', expires_in: 600 }));
    await page.route('**/api/admin/v1/cpa/logins/st1', (route) => json(route, loginStatus));
    await page.reload();
    await page.waitForSelector('#cpa-panel:not([hidden])');
    assert.equal(await page.isHidden('#cpa-paste'), true, 'no login, nothing to paste into');
    assert.match(await page.innerText('#cpa-accounts'), /No account connected/);
    // No domain: the login comes back to http://localhost, so the paste box is there from the start.
    await page.click('#cpa-connect-openai');
    await page.waitForSelector('#cpa-login:not([hidden])');
    assert.equal(await page.isVisible('#cpa-paste'), true, 'no domain: the paste step shows');
    assert.equal(await page.getAttribute('#cpa-login-link', 'href'), 'https://login.example.test/authorize');
    await page.click('#cpa-cancel');
    assert.equal(await page.isHidden('#cpa-login'), true);
    assert.equal(await page.isHidden('#cpa-paste'), true);
    // A domain, the login still going: the paste step stays away.
    hostname = 'askbar.example.com';
    await page.click('#cpa-connect-anthropic');
    await page.waitForSelector('#cpa-login:not([hidden])');
    assert.equal(await page.isHidden('#cpa-paste'), true, 'a domain is set and the login is not over: no paste step');
    // The login does not finish: the step comes back as the way to finish by hand.
    loginStatus = { status: 'error', message: 'timed out' };
    await page.waitForSelector('#cpa-paste:not([hidden])', { timeout: (pollSecs + 5) * 1000 });
    assert.match(await page.innerText('#cpa-message'), /did not finish: timed out/);
    await page.click('#cpa-cancel');
    // A domain and a login that finishes by itself: the box closes and the step never shows.
    loginStatus = { status: 'ok', provider_id: null };
    await page.click('#cpa-connect-anthropic');
    await page.waitForSelector('#cpa-login:not([hidden])');
    assert.equal(await page.isHidden('#cpa-paste'), true);
    await page.waitForSelector('#cpa-login', { state: 'hidden', timeout: (pollSecs + 5) * 1000 });
    assert.equal(await page.isHidden('#cpa-paste'), true, 'a domain and no login pending: hidden');
    await page.unroute('**/api/admin/v1/network');
    await page.unroute('**/api/admin/v1/cpa');
    await page.unroute('**/api/admin/v1/cpa/logins');
    await page.unroute('**/api/admin/v1/cpa/logins/st1');
    await page.reload();
    await page.waitForSelector('tr[data-name="mock"]');
  });
  await step('delete asks first, then removes the provider', async () => {
    await page.click('tr[data-name="dead"] [data-action="edit"]');
    await page.waitForSelector('#provider-panel:not([hidden])');
    assert.match(await page.innerText('#provider-panel-title'), /Edit dead/);
    await page.click('#provider-delete');
    await page.waitForSelector('dialog[open] >> text=Delete dead?');
    await page.click('dialog[open] [data-confirm-cancel]');
    assert.equal(await page.locator('tr[data-name="dead"]').count(), 1);
    await page.click('#provider-delete');
    await page.waitForSelector('dialog[open] >> text=Delete dead?');
    await page.click('dialog[open] [data-confirm-action]');
    await page.waitForFunction(() => !document.querySelector('tr[data-name="dead"]'));
    await panelClosed();
  });
  await step('opening the Providers page asks no provider for its models', async () => {
    const asked = [];
    const onReq = (r) => { if (r.method() === 'POST' && /\/api\/admin\/v1\/providers\/models/.test(r.url())) asked.push(r.url()); };
    page.on('request', onReq);
    await page.goto(base + 'providers/');
    await page.waitForSelector('tr[data-name="mock"]');
    await page.waitForLoadState('networkidle');
    page.off('request', onReq);
    assert.deepEqual(asked, []);
    assert.match(await page.innerText('#catalog-offered'), /Not loaded/);
  });
  await step('#add-provider in the address opens the Add panel, and leaves the address clean', async () => {
    await page.goto(base + 'providers/#add-provider');
    await page.waitForSelector('#provider-panel:not([hidden])');
    assert.equal(await page.innerText('#provider-panel-title'), 'Add provider');
    assert.equal(await page.innerText('#provider-submit'), 'Add and test');
    assert.equal(new URL(page.url()).hash, '', 'the address no longer asks for it, so a reload does not reopen it');
    await page.click('#provider-cancel');
    await panelClosed();
    // From the same page (a search result, a link) the same address opens it again.
    await page.evaluate(() => { location.hash = '#add-provider'; });
    await page.waitForSelector('#provider-panel:not([hidden])');
    await page.click('#provider-cancel');
    await panelClosed();
  });
  await step('Services: the six services show whether they answer, and Restart asks first and then acts', async () => {
    // No controller (ADR 0005): each state comes from the service. Here the server, the gateway and the admin run;
    // Caddy and CLIProxyAPI do not.
    await openPage('services');
    await page.waitForSelector('#services tr[data-name="cpa"]');
    assert.equal(await page.locator('#cpa-panel, #cpa-accounts, #cpa-connect-anthropic').count(), 0, 'the Claude and ChatGPT accounts moved to Providers');
    assert.deepEqual(await page.$$eval('#services tr[data-name]', (c) => c.map((x) => x.dataset.name)), ['server', 'model-gateway', 'alerter', 'admin', 'caddy', 'cpa']);
    assert.equal(await page.locator('#services-recent').count(), 0, 'there is no list of recent restarts');
    // Every visible cell has a visible header over it, and a service name never breaks mid-word.
    const misaligned = await page.evaluate(() => {
      const shown = (e) => e.getClientRects().length > 0;
      const heads = [...document.querySelectorAll('#services-table thead th')].filter(shown);
      const out = [];
      for (const tr of document.querySelectorAll('#services tr[data-name]')) {
        const cells = [...tr.children].filter(shown);
        if (cells.length !== heads.length) { out.push(tr.dataset.name + ': ' + cells.length + ' cells, ' + heads.length + ' headers'); continue; }
        cells.forEach((c, i) => { const a = c.getBoundingClientRect(); const h = heads[i].getBoundingClientRect(); if (Math.abs(a.left - h.left) > 1) out.push(tr.dataset.name + ' column ' + i); });
        const n = tr.querySelector('.name');
        const cell = n.closest('td');
        if (getComputedStyle(n).overflowWrap !== 'normal' || getComputedStyle(n).wordBreak !== 'normal') out.push(tr.dataset.name + ' name may break mid-word');
        if (cell.scrollWidth > cell.clientWidth + 1) out.push(tr.dataset.name + ' name does not fit');
      }
      return out;
    });
    assert.deepEqual(misaligned, []);
    // The alerter runs here too (ADR 0055), so it joins the list between the gateway and the admin.
    assert.match(await page.innerText('#services tr[data-name="alerter"]'), /running/i);
    // Its daily release check asked the stand-in GitHub, with a fixed User-Agent and nothing else about the install, and the row says so (ADR 0055).
    await page.waitForFunction(() => /Askbar is up to date \(/.test(document.querySelector('#services tr[data-name="alerter"]').innerText), null, { timeout: 15000 });
    const releaseRequests = JSON.parse(await new Promise((resolve, reject) => { http.get({ host: '127.0.0.1', port: releasesPort, path: '/_seen' }, (res) => { let b = ''; res.on('data', (c) => (b += c)); res.on('end', () => resolve(b)); }).on('error', reject); }));
    assert.ok(releaseRequests.length >= 1 && releaseRequests.every((r) => r.user_agent === 'askbar-release-check' && r.headers.every((h) => ['host', 'user-agent', 'accept'].includes(h))), JSON.stringify(releaseRequests));
    assert.deepEqual(await page.$$eval('#services tr[data-name="alerter"] button', (b) => b.map((x) => x.dataset.action)), ['restart']);
    assert.match(await page.innerText('#services tr[data-name="server"]'), /running/i);
    // The gateway runs here too, and it has a Restart button like the rest.
    assert.match(await page.innerText('#services tr[data-name="model-gateway"]'), /running/i);
    assert.deepEqual(await page.$$eval('#services tr[data-name="model-gateway"] button', (b) => b.map((x) => x.dataset.action)), ['restart']);
    assert.match(await page.innerText('#services tr[data-name="caddy"]'), /unreachable/i);
    assert.deepEqual(await page.$$eval('#services tr[data-name="server"] button', (b) => b.map((x) => x.dataset.action)), ['restart'], 'only Restart: there is no start or stop');
    // Restart asks first, in the row: the question opens under it, and the row's own button hides until it closes.
    const restartBtn = '#services tr[data-name="server"] [data-action="restart"]';
    const ask = '#services .row-confirm-row';
    await page.click(restartBtn);
    await page.waitForSelector(ask);
    assert.match(await page.innerText(ask + ' .row-confirm-text'), /^Restart Server\? .*Replies being written end as interrupted/);
    assert.equal(await page.isHidden(restartBtn), true, 'the row\'s own button hides while it asks');
    assert.deepEqual(await page.$$eval(ask + ' button', (b) => b.map((x) => x.textContent)), ['Cancel', 'Restart']);
    // A refresh, even a forced one, with a service changing state meanwhile, leaves the question open.
    let gatewayDown = false;
    await page.route('**/api/admin/v1/services', async (route) => {
      const res = await route.fetch({ headers: { ...route.request().headers(), 'if-none-match': '' } });
      const body = await res.json();
      if (gatewayDown) for (const s of body.services) if (s.name === 'model-gateway') s.state = 'exited';
      await route.fulfill({ response: res, json: body, headers: { ...res.headers(), etag: '"x' + Date.now() + '"' } });
    });
    gatewayDown = true;
    const every = await page.evaluate(async () => (await window.ui.ready).STATUS_EVERY_SECS);
    await page.waitForFunction(() => /exited/i.test(document.querySelector('#services tr[data-name="model-gateway"]').innerText), null, { timeout: (every + 10) * 1000 });
    assert.equal(await page.locator(ask).count(), 1, 'a refresh does not close the question');
    assert.equal(await page.isHidden(restartBtn), true);
    assert.equal(await page.locator('#services tr[data-name="server"] + .row-confirm-row').count(), 1, 'it stays right under its row');
    await page.unroute('**/api/admin/v1/services');
    // Cancel closes it, brings the button back and focuses it; so does Esc.
    await page.click(ask + ' [data-row-cancel]');
    assert.equal(await page.locator(ask).count(), 0);
    assert.equal(await page.isVisible(restartBtn), true);
    assert.equal(await page.evaluate(() => document.activeElement && document.activeElement.dataset.action), 'restart', 'Cancel focuses the button again');
    await page.click(restartBtn);
    await page.waitForSelector(ask);
    await page.keyboard.press('Escape');
    assert.equal(await page.locator(ask).count(), 0, 'Esc closes it');
    assert.equal(await page.isVisible(restartBtn), true);
    // CLIProxyAPI is restarted through its supervisor (ADR 0037), so it has a Restart button too. (Not
    // clicked here: there is no supervisor in this test, and the admin would wait a minute for it.)
    assert.deepEqual(await page.$$eval('#services tr[data-name="cpa"] button', (b) => b.map((x) => x.dataset.action)), ['restart']);
    // Confirmed: the attempt is made; Caddy is not running here, so it fails and the page error line says so.
    await page.click('#services tr[data-name="caddy"] [data-action="restart"]');
    await page.waitForSelector(ask);
    await page.click(ask + ' [data-row-action]');
    await page.waitForFunction(() => { const e = document.getElementById('page-error'); return !e.hidden && e.textContent.trim().length > 0; });
    await page.waitForFunction(() => !document.querySelector('#services .row-confirm-row'));
    assert.equal(await page.isVisible('#services tr[data-name="caddy"] [data-action="restart"]'), true);
  });
  await step('clients see the approved models only with a token', async () => {
    // The API is the other listener; without a token it must refuse.
    const r = await fetch(`http://127.0.0.1:${api}/api/client/v1/models`, { headers: { 'x-askbar-client-version': '0.6.0' } });
    assert.equal(r.status, 401);
  });
  await step('people: add a user, generate a pairing key for them, pair with it, then revoke the client', async () => {
    await openPage('people');
    await page.waitForSelector('#users');
    await page.waitForFunction(() => /No clients are paired yet/.test(document.getElementById('clients').textContent));
    // A fresh install has no users, so nothing can be paired yet (ADR 0008): the box says so and cannot make a key.
    await page.waitForFunction(() => /No users yet/.test(document.getElementById('users').textContent));
    await page.click('#pair-open');
    assert.equal(await page.isDisabled('#pair-form button[type=submit]'), true);
    assert.match(await page.innerText('#pair-error'), /Add a user first/);
    await page.click('#pair-cancel');
    await page.click('#add-user');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'u-name', 'the name field has focus');
    assert.equal(await page.getAttribute('#u-name', 'maxlength'), String(await page.evaluate(async () => (await window.ui.ready).NAME_MAX_CHARS)), 'the name limit comes from constants');
    await page.click('#user-form button[type=submit]');
    assert.match(await page.innerText('#u-name-error'), /Enter a name/, 'an empty name is refused where it is typed');
    await page.fill('#u-name', 'Ann');
    await page.click('#user-form button[type=submit]');
    await page.waitForFunction(() => /Ann/.test(document.getElementById('users').textContent));
    await page.click('#pair-open');
    assert.equal(await page.inputValue('#d-user').then((v) => v.length > 0), true, 'the new user is picked');
    assert.equal(await page.isDisabled('#pair-form button[type=submit]'), false);
    await page.fill('#d-name', 'Test laptop');
    await page.click('#pair-form button[type=submit]');
    await page.waitForSelector('#pairing-out:not([hidden])');
    const pairing = (await page.innerText('#pairing-string')).trim();
    assert.match(pairing, /^askbar-pair:/);
    const { url, code } = JSON.parse(Buffer.from(pairing.slice('askbar-pair:'.length), 'base64url').toString());
    assert.equal(url, 'http://localhost:' + api);
    const headers = { 'content-type': 'application/json', 'x-askbar-client-version': '0.6.0' };
    const paired = await fetch(`http://127.0.0.1:${api}/api/client/v1/pairings`, { method: 'POST', headers, body: JSON.stringify({ code }) });
    assert.equal(paired.status, 200);
    const { token } = await paired.json();
    const whoami = () => fetch(`http://127.0.0.1:${api}/api/client/v1/me`, { headers: { ...headers, authorization: 'Bearer ' + token } }).then((r) => r.status);
    assert.equal(await whoami(), 204);
    await page.click('#pairing-done');

    await openPage('providers');
    await openPage('people');
    await page.waitForSelector('#clients [data-client]');
    assert.match(await page.innerText('#clients [data-client]'), /Test laptop[\s\S]*Ann/);
    assert.doesNotMatch(await page.textContent('#clients [data-client] .col-seen'), /never/, 'it has been seen');
    assert.match(await page.innerText('#users [data-user]'), /Ann\s+1\s+Active/, 'the user row counts its client');
    await page.click('#clients [data-action="revoke"]');
    await page.waitForSelector('dialog[open] [data-confirm-action]');
    await page.click('dialog[open] [data-confirm-action]');
    await page.waitForFunction(() => /Revoked/.test(document.querySelector('#clients [data-client]').textContent));
    assert.equal(await page.locator('#clients [data-action="revoke"]').count(), 0);
    assert.equal(await whoami(), 401);
  });
  await step('users: Pause is the emergency stop for one person, shown as a word and a symbol, and Resume undoes it', async () => {
    await openPage('people');
    await page.waitForSelector('#users [data-user]');
    const row = '#users [data-user]';
    assert.equal(await page.innerText(row + ' .name'), 'Ann');
    assert.equal(await page.innerText(row + ' [data-action="pause"]'), 'Pause');
    assert.equal(await page.getAttribute(row + ' [data-action="pause"]', 'aria-label'), 'Pause replies for Ann');
    await page.click(row + ' [data-action="pause"]');
    await page.waitForFunction(() => /Replies paused/.test(document.querySelector('#users [data-user]').textContent));
    assert.equal(await page.innerText(row + ' [data-action="resume"]'), 'Resume');
    assert.equal(await page.locator(row + ' .status-paused').count(), 1, 'a word and a symbol');
    // It is on the server's side too: the admin API says so, and a reload shows it.
    await openPage('providers');
    await openPage('people');
    await page.waitForFunction(() => /Replies paused/.test(document.querySelector('#users [data-user]').textContent));
    await page.click(row + ' [data-action="resume"]');
    await page.waitForFunction(() => /Active/.test(document.querySelector('#users [data-user]').textContent));
    assert.equal(await page.innerText(row + ' [data-action="pause"]'), 'Pause');
    assert.equal(await page.locator('#users b').count(), 0, 'names are text');
  });
  await step('users: the row menu has Edit and Delete; Edit opens the side panel with the usage limits greyed, reachable by Tab and inert on Enter', async () => {
    await openPage('people');
    await page.waitForSelector('#users [data-user]');
    const more = '#users [data-user] .row-menu-button';
    assert.equal(await page.getAttribute(more, 'aria-label'), 'More actions for Ann');
    await page.click(more);
    assert.deepEqual(await page.$$eval('#users [data-user] .menu [role="menuitem"]', (b) => b.map((x) => x.textContent.trim())), ['Edit', 'Delete']);
    await page.click('#users [data-user] .menu [data-action="edit"]');
    await page.waitForSelector('#user-panel:not([hidden])');
    assert.equal(await page.innerText('#user-panel-title'), 'Edit Ann');
    // The name and the usage limits are labels only: no number, no field, aria-disabled, never the disabled attribute.
    const limits = await page.$$eval('#edit-form .na', (n) => n.map((x) => ({ text: x.textContent.replace(/\s+/g, ' ').trim(), aria: x.getAttribute('aria-disabled'), tab: x.tabIndex, tip: x.dataset.tip, controls: x.querySelectorAll('input, select, textarea, button').length })));
    assert.equal(limits.length, 4, 'the name and the three limits');
    for (const l of limits) {
      assert.equal(l.aria, 'true');
      assert.equal(l.tab, 0);
      assert.match(l.tip, /^Not available yet: /);
      assert.equal(l.controls, 0, 'no field behind it');
      assert.doesNotMatch(l.text.replace(/Not available yet:.*$/, ''), /\d/, 'no number on a greyed item: ' + l.text);
    }
    assert.match(limits[1].text, /Monthly token cap/);
    assert.equal(await page.locator('#edit-form [disabled]').count(), 0, 'aria-disabled, not disabled');
    // Reachable by Tab, and Enter does nothing.
    const snapshot = () => page.evaluate(() => ({ url: location.href, hidden: document.getElementById('user-panel').hidden, paused: document.getElementById('e-paused').checked, boxes: document.querySelectorAll('dialog[open]').length }));
    await page.focus('#edit-form .na >> nth=1');
    assert.match(await page.evaluate(() => document.activeElement.dataset.tip), /^Not available yet: the server does not track usage or caps per person\.$/);
    const before = await snapshot();
    await page.keyboard.press('Enter');
    await page.keyboard.press('Space');
    assert.deepEqual(await snapshot(), before, 'nothing happened');
    let reached = 0;
    await page.focus('#edit-form .na >> nth=0');
    for (let i = 0; i < 6; i++) { await page.keyboard.press('Tab'); if (await page.evaluate(() => !!document.activeElement.closest('.na'))) reached++; }
    assert.equal(reached, 3, 'Tab reaches the three other greyed items');
    // Pause from the panel, then resume from it.
    await page.check('#e-paused');
    await page.click('#edit-save');
    await page.waitForSelector('#user-panel', { state: 'hidden' });
    await page.waitForFunction(() => /Replies paused/.test(document.querySelector('#users [data-user]').textContent));
    await page.waitForFunction(() => !location.search.includes('panel='));
    await page.click(more);
    await page.click('#users [data-user] .menu [data-action="edit"]');
    await page.waitForSelector('#user-panel:not([hidden])');
    assert.equal(await page.isChecked('#e-paused'), true);
    await page.uncheck('#e-paused');
    await page.click('#edit-save');
    await page.waitForSelector('#user-panel', { state: 'hidden' });
    await page.waitForFunction(() => /Active/.test(document.querySelector('#users [data-user]').textContent));
    await page.waitForFunction(() => !location.search.includes('panel='));
  });
  await step('users: Delete asks for the name and stays disabled until it matches; Pause instead pauses; then it removes the person and nothing else', async () => {
    await openPage('people');
    await page.waitForSelector('#users [data-user]');
    await page.click('#add-user');
    await page.fill('#u-name', 'Zed');
    await page.click('#user-form button[type=submit]');
    await page.waitForFunction(() => /Zed/.test(document.getElementById('users').textContent));
    const zed = '#users [data-user]:has(.name:text-is("Zed"))';
    const openDelete = async () => {
      await page.click(zed + ' .row-menu-button');
      await page.click(zed + ' .menu [data-action="delete"]');
      await page.waitForSelector('dialog[open] [data-confirm-action]');
    };
    await openDelete();
    const box = 'dialog[open]';
    assert.match(await page.innerText(box), /cannot be undone/);
    assert.match(await page.innerText(box + ' .box-title'), /Delete Zed\?/);
    const go = box + ' [data-confirm-action]';
    assert.equal(await page.isDisabled(go), true, 'nothing to confirm yet');
    await page.fill(box + ' input', 'zed');
    assert.equal(await page.isDisabled(go), true, 'the name must match exactly');
    // Cancel closes it and deletes nothing.
    await page.click(box + ' [data-confirm-cancel]');
    assert.equal(await page.locator('dialog[open]').count(), 0);
    assert.equal(await page.locator(zed).count(), 1);
    // Pause instead: the box closes, nothing is deleted, the person is paused.
    await openDelete();
    await page.click(box + ' [data-confirm-alt]');
    await page.waitForFunction(() => [...document.querySelectorAll('#users [data-user]')].some((r) => /Zed/.test(r.textContent) && /Replies paused/.test(r.textContent)));
    assert.equal(await page.locator('dialog[open]').count(), 0);
    // A paused person has no "Pause instead" to offer.
    await openDelete();
    assert.equal(await page.locator(box + ' [data-confirm-alt]').count(), 0);
    await page.fill(box + ' input', 'Zed');
    assert.equal(await page.isDisabled(go), false);
    await page.click(go);
    await page.waitForFunction(() => !/Zed/.test(document.getElementById('users').textContent));
    assert.match(await page.innerText('#users'), /Ann/, 'the other person stays');
    const left = await page.evaluate(async () => (await (await fetch('/api/admin/v1/users')).json()).map((u) => u.name));
    assert.ok(left.includes('Ann') && !left.includes('Zed'), JSON.stringify(left));
  });
  await step('users: Delete in the Edit panel asks the same way', async () => {
    await openPage('people');
    await page.waitForSelector('#users [data-user]');
    await page.click('#users [data-user] .row-menu-button');
    await page.click('#users [data-user] .menu [data-action="edit"]');
    await page.waitForSelector('#user-panel:not([hidden])');
    await page.click('#edit-delete');
    await page.waitForSelector('dialog[open] [data-confirm-action]');
    assert.equal(await page.isDisabled('dialog[open] [data-confirm-action]'), true);
    await page.click('dialog[open] [data-confirm-cancel]');
    assert.equal(await page.locator('#users [data-user]').count(), 1, 'nothing was deleted');
    await page.click('#user-panel [data-panel-close]');
    await page.waitForSelector('#user-panel', { state: 'hidden' });
  });
  await step('people: #add-user and #pair-open in the address open their box and leave the address clean', async () => {
    for (const [hash, box] of [['#add-user', 'user-box'], ['#pair-open', 'pair-box']]) {
      await page.goto(base + 'people/' + hash);
      await page.waitForSelector('#' + box + '[open]');
      await page.waitForFunction(() => location.hash === '');
      await page.keyboard.press('Escape');
      await page.waitForSelector('#' + box, { state: 'hidden' });
    }
  });
  await step('live lists: new rows are highlighted and announced, then calm; scroll, focus and typing survive; an unchanged answer changes nothing', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    await openPage('people');
    await page.waitForSelector('#users [data-user]');
    // The admin's users, plus whatever the test adds to the answer (no server state is made).
    let extra = [];
    let seen = 0;
    await page.route('**/api/admin/v1/users', async (route) => {
      if (route.request().method() !== 'GET') { await route.continue(); return; }
      seen++;
      const real = await (await route.fetch()).json();
      await route.fulfill({ contentType: 'application/json', body: JSON.stringify([...real, ...extra]) });
    });
    // A hidden tab holds the lists; a visible one asks at once. (Emulated: the page reports hidden, then visible.)
    const visibility = (hidden) => page.evaluate((h) => {
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => h });
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (h ? 'hidden' : 'visible') });
      document.dispatchEvent(new Event('visibilitychange'));
    }, hidden);
    const refresh = async () => {
      const before = seen;
      await visibility(true);
      await page.waitForTimeout(100);
      assert.equal(seen, before, 'a hidden tab does not ask for the list');
      await visibility(false);
      for (let i = 0; i < 50 && seen === before; i++) await page.waitForTimeout(100);
      assert.ok(seen > before, 'a tab that is visible again asks at once');
      await page.waitForTimeout(300); // the answer is drawn
    };
    try {
      const fake = (n) => ({ id: 'live-' + n, name: 'Live ' + n + ' <b>x</b>', paused: false });
      // Room to scroll, and a control in the first row to focus.
      await page.evaluate(() => { const d = document.createElement('div'); d.id = 't-tall'; d.style.height = '3000px'; document.getElementById('main').append(d); });
      extra = [fake(1)];
      await refresh();
      assert.equal(await page.locator('#users [data-user="live-1"]').count(), 1, 'the new row is there');
      assert.equal(await page.locator('#users [data-user="live-1"]').evaluate((n) => n.classList.contains('row-new')), true, 'and highlighted');
      assert.equal(await page.locator('#users b').count(), 0, 'names are text');
      assert.equal(await page.innerText('#announcer'), '1 new user');
      await page.waitForTimeout(K.HIGHLIGHT_SECS * 1000 + 500);
      assert.equal(await page.locator('#users .row-new').count(), 0, 'calm after HIGHLIGHT_SECS');
      // Focus a button in a row and scroll; a refresh with another new row keeps both, on the same node.
      await page.focus('#users [data-user="live-1"] button');
      await page.evaluate(() => { document.activeElement.dataset.mark = 'same'; window.scrollTo(0, 400); });
      // (A row added above the view makes the browser move scrollY to keep what is on screen still, so what stays is the
      // place on screen of what is in view, here the block that gives the page room to scroll.)
      const place = () => page.evaluate(() => Math.round(document.getElementById('t-tall').getBoundingClientRect().top));
      const y = await place();
      const scrolled = await page.evaluate(() => window.scrollY);
      extra = [fake(1), fake(2)];
      await refresh();
      assert.equal(await page.locator('#users [data-user="live-2"].row-new').count(), 1);
      assert.equal(await page.evaluate(() => document.activeElement.dataset.mark), 'same', 'the focused control is the same node');
      // Either the scroll position is the same, or the browser kept what is on screen still: the view does not jump.
      assert.ok((await page.evaluate(() => window.scrollY)) === scrolled || (await place()) === y, 'the view does not jump');
      // A delete confirmation that is open keeps its typing while the list changes under it.
      await page.click('#users [data-user="live-1"] .row-menu-button');
      await page.click('#users [data-user="live-1"] .menu [data-action="delete"]');
      await page.fill('dialog[open] input', 'Liv');
      extra = [fake(1), fake(2), fake(3)];
      await refresh();
      assert.equal(await page.inputValue('dialog[open] input'), 'Liv', 'typing is kept');
      assert.equal(await page.evaluate(() => document.activeElement.tagName), 'INPUT', 'and so is focus');
      await page.click('dialog[open] [data-confirm-cancel]');
      // The same answer again: nothing in the page changes.
      await page.evaluate(() => {
        window.__muts = 0;
        const o = new MutationObserver((m) => { window.__muts += m.length; });
        o.observe(document.getElementById('users'), { subtree: true, childList: true, attributes: true, characterData: true });
        o.observe(document.getElementById('clients'), { subtree: true, childList: true, attributes: true, characterData: true });
      });
      await refresh();
      await refresh();
      assert.equal(await page.evaluate(() => window.__muts), 0, 'an unchanged answer changes nothing');
    } finally {
      await page.unroute('**/api/admin/v1/users');
      await page.evaluate(() => { delete document.hidden; delete document.visibilityState; const d = document.getElementById('t-tall'); if (d) d.remove(); });
    }
    await openPage('providers');
  });
  await step('live lists: a 304 changes nothing, and the ETag is sent back', async () => {
    await openPage('people');
    await page.waitForSelector('#users [data-user]');
    const sent = [];
    await page.route('**/api/admin/v1/clients', async (route) => {
      const inm = route.request().headers()['if-none-match'];
      sent.push(inm || null);
      if (inm === '"v1"') { await route.fulfill({ status: 304, headers: { etag: '"v1"' } }); return; }
      const real = await route.fetch();
      await route.fulfill({ response: real, headers: { ...real.headers(), etag: '"v1"' } });
    });
    try {
      await page.reload();
      await page.waitForSelector('#app:not([hidden])');
      await page.waitForFunction(() => document.querySelector('#clients [data-client], #clients .empty-row'));
      await page.evaluate(() => {
        window.__muts = 0;
        new MutationObserver((m) => { window.__muts += m.length; }).observe(document.getElementById('clients'), { subtree: true, childList: true, attributes: true, characterData: true });
        Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
        document.dispatchEvent(new Event('visibilitychange'));
        Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
        document.dispatchEvent(new Event('visibilitychange'));
      });
      for (let i = 0; i < 50 && sent.length < 2; i++) await page.waitForTimeout(100);
      await page.waitForTimeout(300);
      assert.deepEqual(sent.slice(0, 2), [null, '"v1"'], 'the second ask carries the ETag of the first answer');
      assert.equal(await page.evaluate(() => window.__muts), 0, 'a 304 changes nothing');
    } finally {
      await page.unroute('**/api/admin/v1/clients');
      await page.evaluate(() => { delete document.hidden; });
    }
    await openPage('providers');
  });
  await step('live lists: the interval follows the tab, a lost connection backs off along BACKOFF_SECS and says Reconnecting…, and the first answer clears it', async () => {
    const out = await page.evaluate(async () => {
      const K = await window.ui.ready;
      const calls = [];
      const timer = { set: (fn, ms) => { calls.push(ms); return calls.length; }, clear: () => {} };
      let down = true;
      const slot = document.getElementById('page-error');
      const live = window.ui.live(async () => { if (down) throw new TypeError('Failed to fetch'); return { n: 1 }; }, { render: () => {}, notice: slot, timer });
      const result = { K, delays: [], noteWhileDown: null, noteAfter: null };
      await live.start();
      result.delays.push(calls[calls.length - 1]);
      result.noteWhileDown = document.querySelector('.live-note') && document.querySelector('.live-note').textContent;
      result.polite = document.querySelector('.live-note').getAttribute('role');
      for (let i = 0; i < 5; i++) { await live.refresh(); result.delays.push(calls[calls.length - 1]); }
      down = false;
      await live.refresh();
      result.afterOk = calls[calls.length - 1];
      result.noteAfter = document.querySelector('.live-note');
      // A refusal is an answer, not a lost connection: no backoff, no notice.
      down = false;
      const refused = window.ui.live(async () => { const e = new Error('no'); e.status = 500; throw e; }, { render: () => {}, notice: slot, timer, onError: () => {} });
      await refused.start();
      result.refusedDelay = calls[calls.length - 1];
      result.refusedNote = !!document.querySelector('.live-note');
      // The tab hides: a list pauses (no timer), the status kind goes on at HIDDEN_EVERY_SECS.
      const hide = (h) => { Object.defineProperty(document, 'hidden', { configurable: true, get: () => h }); document.dispatchEvent(new Event('visibilitychange')); };
      const pause = window.ui.live(async () => ({ n: 1 }), { render: () => {}, timer });
      const slow = window.ui.live(async () => ({ n: 1 }), { render: () => {}, timer, hidden: 'slow' });
      await pause.start(); await slow.start();
      hide(true);
      result.hiddenPause = pause.state.delay;
      result.hiddenSlow = slow.state.delay;
      result.visibleSlowEvery = null;
      const named = window.ui.live(async () => ({ n: 1 }), { render: () => {}, timer, every: 'STATUS_EVERY_SECS' });
      hide(false);
      await named.start();
      result.visible = pause.state.delay;
      result.named = named.state.delay;
      for (const l of [live, refused, pause, slow, named]) l.stop();
      delete document.hidden;
      return result;
    });
    const K = out.K;
    assert.deepEqual(K.BACKOFF_SECS, [10, 20, 40, 60]);
    assert.deepEqual(out.delays, [10000, 20000, 40000, 60000, 60000, 60000], 'the schedule is BACKOFF_SECS, the last step repeating');
    assert.match(out.noteWhileDown, /Reconnecting…/);
    assert.equal(out.polite, 'status', 'announced politely, not as an alert');
    assert.equal(out.afterOk, K.LIVE_EVERY_SECS * 1000, 'back to the normal interval after the first answer');
    assert.equal(out.noteAfter, null, 'the notice clears');
    assert.equal(out.refusedDelay, K.LIVE_EVERY_SECS * 1000, 'an answer, even a refusal, does not back off');
    assert.equal(out.refusedNote, false);
    assert.equal(out.hiddenPause, null, 'a hidden tab holds the list');
    assert.equal(out.hiddenSlow, K.HIDDEN_EVERY_SECS * 1000, 'the bell and title go on at HIDDEN_EVERY_SECS');
    assert.equal(out.visible, K.LIVE_EVERY_SECS * 1000);
    assert.equal(out.named, K.STATUS_EVERY_SECS * 1000);
  });
  await step('live lists: a failed check shows Reconnecting… next to the list and clears when the answer comes back', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    await openPage('people');
    await page.waitForSelector('#users [data-user]');
    let down = true;
    await page.route('**/api/admin/v1/clients', async (route) => { if (down) await route.abort(); else await route.continue(); });
    try {
      await page.evaluate(() => {
        Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
        document.dispatchEvent(new Event('visibilitychange'));
        Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
        document.dispatchEvent(new Event('visibilitychange'));
      });
      await page.waitForSelector('.live-note');
      assert.match(await page.innerText('.live-note'), /Reconnecting…/);
      assert.equal(await page.locator('#banners [data-banner="reconnecting"]').count(), 0, 'not a banner');
      assert.equal(await page.locator('#users [data-user]').count() > 0, true, 'the list stays as it was');
      down = false;
      // The next check is BACKOFF_SECS[0] away; a tab that comes back asks at once.
      await page.evaluate(() => {
        Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
        document.dispatchEvent(new Event('visibilitychange'));
        Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
        document.dispatchEvent(new Event('visibilitychange'));
      });
      await page.waitForSelector('.live-note', { state: 'detached', timeout: K.BACKOFF_SECS[0] * 1000 });
    } finally {
      // The aborted request is the point of the step: the browser logs it, so it is not a page error.
      for (let i = errors.length - 1; i >= 0; i--) if (errors[i] === 'Failed to load resource: net::ERR_FAILED') errors.splice(i, 1);
      await page.unroute('**/api/admin/v1/clients');
      await page.evaluate(() => { delete document.hidden; });
    }
    await openPage('providers');
  });
  await step('live lists: a client that pairs turns the pairing box to Connected; the tab title carries the alert count; polling is not an action and does not extend the sign-in', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    await openPage('people');
    await page.waitForSelector('#users [data-user]');
    await page.click('#pair-open');
    await page.selectOption('#d-user', { label: 'Ann' });
    await page.fill('#d-name', 'Live laptop');
    await page.click('#pair-form button[type=submit]');
    await page.waitForSelector('#pairing-out:not([hidden])');
    assert.match(await page.innerText('#pairing-status'), /Waiting for the app to pair/);
    assert.match(await page.innerText('#pairing-meta'), new RegExp('expires in ' + K.PAIR_CODE_TTL_SECS / 60 + ' minutes'), 'the time limit is the constant');
    const pairing = (await page.innerText('#pairing-string')).trim();
    const { code } = JSON.parse(Buffer.from(pairing.slice('askbar-pair:'.length), 'base64url').toString());
    const headers = { 'content-type': 'application/json', 'x-askbar-client-version': '0.6.0' };
    const paired = await fetch(`http://127.0.0.1:${api}/api/client/v1/pairings`, { method: 'POST', headers, body: JSON.stringify({ code }) });
    assert.equal(paired.status, 200);
    // The next check (or the admin's own change) shows it; nothing is clicked.
    await page.waitForFunction(() => /Connected/.test(document.getElementById('pairing-status').textContent), null, { timeout: 30000 });
    assert.match(await page.innerText('#pairing-status'), /Connected[\s\S]*Live laptop is paired/);
    assert.equal(await page.isVisible('#pairing-key'), false, 'the key is gone: it no longer works');
    assert.equal(await page.innerText('#pairing-again'), 'Pair another client');
    assert.equal(await page.locator('#clients [data-client]:has-text("Live laptop")').count(), 1, 'and the list has it');
    assert.equal(await page.$eval('#pairing-status .status', (n) => n.classList.contains('status-ok')), true, 'a word and a symbol');
    await page.click('#pairing-done');
    // (That a read is not an admin action and does not extend the sign-in is pinned in Rust: tests/actions.rs, tests/admin_auth.rs.)
    await page.click('#clients [data-client]:has-text("Live laptop") [data-action="revoke"]');
    await page.waitForSelector('dialog[open] [data-confirm-action]');
    await page.click('dialog[open] [data-confirm-action]');
    await page.waitForSelector('#clients [data-client]:has-text("Live laptop"):has-text("Revoked")');
  });
  await step('the tab title carries the alert count', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    const clean = { server_online: true, token_rejected: false, audit_chain: { state: 'ok' }, alerts: 'ok', sealed_content: { open: false, alerts: [] } };
    let fake = { ...clean, server_online: false };
    await page.route('**/api/admin/v1/status', async (route) => {
      const real = await (await route.fetch()).json();
      await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ ...real, ...fake }) });
    });
    try {
      await page.goto(base + 'network/');
      await page.waitForSelector('#app:not([hidden])');
      await page.waitForFunction(() => /^\(1\) Network/.test(document.title), null, { timeout: K.STATUS_EVERY_SECS * 3000 });
      assert.match(await page.title(), /^\(1\) Network · Askbar admin$/);
      // The condition ends: the count leaves the title. (The hidden-tab interval for the status is checked above, with ui.live.)
      fake = clean;
      await page.waitForFunction(() => document.title === 'Network · Askbar admin', null, { timeout: K.STATUS_EVERY_SECS * 3000 });
    } finally {
      await page.unroute('**/api/admin/v1/status');
    }
    await page.goto(base + 'providers/');
    await page.waitForSelector('#app:not([hidden])');
  });
  await step('settings page saves the system prompt and display defaults', async () => {
    await openPage('general');
    await page.waitForSelector('#settings-form');
    await page.waitForFunction(() => /quick-question popup/.test(document.getElementById('s-prompt').value));
    await page.fill('#s-prompt', 'You are a pirate.');
    await page.selectOption('#s-theme', 'dark');
    await page.fill('#s-width', '900');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    assert.match(await page.locator('#toasts .toast').last().innerText(), /Settings saved/);
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-width').value === '900');
    assert.equal(await page.inputValue('#s-prompt'), 'You are a pirate.');
    assert.equal(await page.inputValue('#s-theme'), 'dark');
  });
  await step('settings: the privacy notice and the AI warning are on with their default words, and the changes are kept', async () => {
    await openSettings();
    await page.waitForFunction(() => /AI can make mistakes/.test(document.getElementById('s-warning-text').value));
    assert.equal(await page.isChecked('#s-notice-on'), true);
    assert.equal(await page.isChecked('#s-warning-on'), true);
    assert.match(await page.inputValue('#s-notice-text'), /sealed copy of each goes to the audit record/);
    await page.fill('#s-notice-text', 'We keep <b>everything</b> for the number of days below.');
    await page.uncheck('#s-warning-on');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    assert.match(await page.locator('#toasts .toast').last().innerText(), /Settings saved/);
    await openSettings();
    await page.waitForFunction(() => /everything/.test(document.getElementById('s-notice-text').value));
    assert.equal(await page.inputValue('#s-notice-text'), 'We keep <b>everything</b> for the number of days below.', 'text, never markup');
    assert.equal(await page.isChecked('#s-warning-on'), false);
    assert.equal(await page.isChecked('#s-notice-on'), true);
    // Put it back: the default words, and both on.
    await page.fill('#s-notice-text', '');
    await page.check('#s-warning-on');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    await openSettings();
    await page.waitForFunction(() => /sealed copy of each goes to the audit record/.test(document.getElementById('s-notice-text').value));
  });
  await step('an invalid setting shows the server message', async () => {
    await page.fill('#s-top', '0.5');
    await page.fill('#s-width', '100');
    await page.evaluate(() => document.getElementById('settings-form').noValidate = true);
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#settings-error:not([hidden])');
    assert.match(await page.innerText('#settings-error'), /Width must be/);
  });
  await step('generation timeout: defaults to 300, saves 120 across a reload, refuses 29', async () => {
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-gen-timeout').value === '300');
    assert.equal(await page.getAttribute('#s-gen-timeout', 'min'), '30');
    assert.equal(await page.getAttribute('#s-gen-timeout', 'max'), '3600');
    assert.equal(await page.innerText('#s-gen-timeout-min'), '', 'the default is already in the hint, so nothing repeats it');
    await page.fill('#s-width', '900'); // the previous step left an invalid width in the form
    await page.fill('#s-gen-timeout', '120');
    assert.equal(await page.innerText('#s-gen-timeout-min'), '120 seconds = 2 minutes');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    assert.match(await page.locator('#toasts .toast').last().innerText(), /Settings saved/);
    await page.fill('#s-gen-timeout', '777'); // unsaved: only a reload from the server brings back 120
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-gen-timeout').value === '120');
    assert.equal(await page.inputValue('#s-gen-timeout'), '120');
    await page.fill('#s-gen-timeout', '90');
    assert.equal(await page.innerText('#s-gen-timeout-min'), '90 seconds = 1.5 minutes');

    await page.fill('#s-gen-timeout', '29');
    await page.evaluate(() => document.getElementById('settings-form').noValidate = true);
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#settings-error:not([hidden])');
    assert.match(await page.innerText('#settings-error'), /between 30 and 3600/);
    await page.fill('#s-gen-timeout', '555'); // so the wait below can only be satisfied by a reload
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-gen-timeout').value === '120');
    assert.equal(await page.inputValue('#s-gen-timeout'), '120', 'the refused value was not saved');
  });
  await step('history sent with each question: 0 (all) by default, saves 4 across a reload', async () => {
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-history-turns').value === '0');
    assert.equal(await page.getAttribute('#s-history-turns', 'min'), '0');
    assert.equal(await page.getAttribute('#s-history-turns', 'max'), '1000');
    await page.fill('#s-history-turns', '4');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    await page.fill('#s-history-turns', '9'); // unsaved: only a reload from the server brings back 4
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-history-turns').value === '4');
  });
  await step('newest first: off by default, saves across a reload', async () => {
    await openSettings();
    assert.equal(await page.isChecked('#s-newest-first'), false);
    await page.check('#s-newest-first');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    await page.uncheck('#s-newest-first'); // unsaved: only a reload from the server brings it back
    await openSettings();
    assert.equal(await page.isChecked('#s-newest-first'), true);
  });
  await step('general: the save bar counts the changed fields, Discard restores them, and Save sends one PUT', async () => {
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-gen-timeout').value !== '');
    const bar = '#settings-form-save-bar';
    const text = async () => page.innerText('#settings-form-save-bar [data-save-text]');
    assert.equal(await page.isVisible(bar), false, 'no bar with nothing changed');
    const prompt = await page.inputValue('#s-prompt');
    const timeout = await page.inputValue('#s-gen-timeout');
    const other = timeout === '130' ? '140' : '130';
    await page.fill('#s-gen-timeout', other);
    await page.waitForSelector(bar + ':not([hidden])');
    assert.equal(await text(), '1 unsaved change: Generation timeout (seconds)');
    await page.fill('#s-prompt', prompt + ' x');
    assert.equal(await text(), '2 unsaved changes');
    await page.fill('#s-prompt', prompt); // reverted by hand
    assert.equal(await text(), '1 unsaved change: Generation timeout (seconds)');
    await page.fill('#s-gen-timeout', timeout); // reverted by hand: back to none
    assert.equal(await page.isVisible(bar), false, 'the bar goes when nothing differs');
    // Discard puts every field back.
    await page.fill('#s-gen-timeout', other);
    await page.fill('#s-prompt', prompt + ' y');
    await page.click(bar + ' [data-save-discard]');
    assert.equal(await page.inputValue('#s-gen-timeout'), timeout);
    assert.equal(await page.inputValue('#s-prompt'), prompt);
    assert.equal(await page.isVisible(bar), false);
    // Save sends one PUT, and the bar clears once the server has the values.
    let puts = 0;
    const count = (r) => { if (r.method() === 'PUT' && r.url().endsWith('/api/admin/v1/settings')) puts += 1; };
    page.on('request', count);
    try {
      await page.fill('#s-gen-timeout', other);
      await page.fill('#s-prompt', prompt + ' z');
      await page.click('#settings-form-save-bar button[type=submit]');
      await page.waitForSelector('#toasts .toast');
      assert.equal(puts, 1, 'one PUT for the whole form');
      assert.equal(await page.isVisible(bar), false, 'saved values are the new base');
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), true, 'no sideways scroll');
    } finally {
      page.off('request', count);
    }
    // Put the values back as they were.
    await page.fill('#s-gen-timeout', timeout);
    await page.fill('#s-prompt', prompt);
    await page.click('#settings-form-save-bar button[type=submit]');
    await page.waitForSelector('#toasts .toast');
  });
  await step('kill switch: off by default; it asks first and Cancel changes nothing; on and off save alone and the status bar follows', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-gen-timeout').value === '120');
    assert.equal(await page.innerText('#s-kill-state'), 'off');
    assert.equal(await page.isChecked('#s-kill-aborts'), true);
    let puts = 0;
    const count = (r) => { if (r.method() === 'PUT' && r.url().endsWith('/api/admin/v1/settings')) puts += 1; };
    page.on('request', count);
    const box = page.locator('dialog.box[open]');
    try {
      await page.click('#s-kill-switch');
      await box.waitFor();
      assert.match(await box.innerText(), /Turn on the kill switch\?/);
      await box.locator('[data-confirm-cancel]').click();
      await box.waitFor({ state: 'detached' });
      assert.equal(await page.innerText('#s-kill-state'), 'off');
      assert.equal(puts, 0, 'Cancel sends nothing');
      // On: the box says what happens to the running replies, the switch saves, and the status bar follows within one poll.
      await page.click('#s-kill-switch');
      await box.waitFor();
      assert.match(await box.innerText(), /The replies running now are stopped\./);
      await box.locator('[data-confirm-action]').click();
      await box.waitFor({ state: 'detached' });
      await page.waitForFunction(() => document.getElementById('s-kill-state').textContent === 'on');
      assert.equal(await page.innerText('#s-kill-switch'), 'Turn off kill switch');
      assert.equal(puts, 1);
      await page.waitForFunction(() => document.querySelector('#sb-kill .status-word').textContent === 'Kill switch: on', null, { timeout: K.STATUS_EVERY_SECS * 3000 });
      // Unsaved edits in the form are not sent by the switch.
      await page.uncheck('#s-kill-aborts');
      await page.waitForSelector('#settings-form-save-bar:not([hidden])');
      await page.click('#s-kill-switch');
      await box.waitFor();
      assert.match(await box.innerText(), /Replies start again for everyone\./);
      await box.locator('[data-confirm-action]').click();
      await box.waitFor({ state: 'detached' });
      await page.waitForFunction(() => document.getElementById('s-kill-state').textContent === 'off');
      await page.waitForFunction(() => document.querySelector('#sb-kill .status-word').textContent === 'Kill switch: off', null, { timeout: K.STATUS_EVERY_SECS * 3000 });
      assert.equal(await page.isChecked('#s-kill-aborts'), false, 'the unsaved choice stays in the form');
      assert.equal(await page.isVisible('#settings-form-save-bar'), true);
      assert.equal(puts, 2);
    } finally {
      page.off('request', count);
      // Whatever failed above, the switch is left off for the later steps.
      await page.evaluate(async () => {
        const h = { 'X-Askbar-Admin': '1', 'Content-Type': 'application/json' };
        const s = await (await fetch('/api/admin/v1/settings', { headers: h })).json();
        if (s.kill_switch) await fetch('/api/admin/v1/settings', { method: 'PUT', headers: h, body: JSON.stringify({ system_prompt: s.system_prompt, display: s.display, kill_switch: false }) });
      });
    }
    // Save the choice (aborts off), then reload: it is kept, and the switch is still off.
    await page.click('#settings-form-save-bar button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-kill-aborts').checked === false);
    assert.equal(await page.innerText('#s-kill-state'), 'off');
    // Back to the default: aborts on, saved.
    await page.check('#s-kill-aborts');
    await page.click('#settings-form-save-bar button[type=submit]');
    await page.waitForSelector('#toasts .toast');
  });
  await step('alerts: not set up shows a banner; ntfy settings are saved, tested and removed, and never shown again (ADR 0055)', async () => {
    const TOPIC = 'e2e-topic-' + 'q7x9k2';
    const TOKEN = 'e2e-token-' + 'm4p8z1';
    const sink = (p, method = 'GET') => new Promise((resolve, reject) => {
      const req = http.request({ host: '127.0.0.1', port: ntfyPort, path: p, method }, (res) => { let b = ''; res.on('data', (c) => (b += c)); res.on('end', () => resolve(b)); });
      req.on('error', reject); req.end();
    });
    await openPage('notifications');
    await page.waitForSelector('#alerts-form');
    await page.waitForSelector('#banners [data-banner="alerts"]');
    assert.match(await page.innerText('#banners [data-banner="alerts"]'), /Alerts are not set up/);
    assert.equal(await page.innerText('#a-held-topic'), 'no');
    // A test with nothing set up says so, and sends nothing.
    await page.click('#a-test');
    await page.waitForFunction(() => /No delivery channel is set up/.test(document.getElementById('a-result').textContent));
    // Save: the fields are secrets, so they are emptied and only a yes/no is shown afterwards.
    await page.fill('#a-server', 'http://127.0.0.1:' + ntfyPort);
    await page.fill('#a-topic', TOPIC);
    await page.fill('#a-token', TOKEN);
    await page.click('#alerts-form button[type=submit]');
    await page.waitForFunction(() => document.getElementById('a-held-topic').textContent === 'yes');
    assert.equal(await page.innerText('#a-held-token'), 'yes');
    assert.equal(await page.inputValue('#a-topic'), '');
    assert.equal(await page.inputValue('#a-token'), '');
    await page.waitForSelector('#banners [data-banner="alerts"]', { state: 'detached' });
    // A test alert is delivered to the sink, on the topic, with the token, and the page says so.
    await page.click('#a-test');
    await page.waitForFunction(() => /ntfy: delivered/.test(document.getElementById('a-result').textContent));
    const seen = JSON.parse(await sink('/_seen'));
    assert.equal(seen.length, 1);
    assert.deepEqual([seen[0].path, seen[0].title, seen[0].authorization], ['/' + TOPIC, 'Askbar test alert', 'Bearer ' + TOKEN]);
    // The sink refuses it: the page says what went wrong in its own words, not the server's.
    await sink('/_status/401', 'POST');
    await page.click('#a-test');
    await page.waitForFunction(() => /the token or login was not accepted/.test(document.getElementById('a-result').textContent));
    await sink('/_status/200', 'POST');
    // Nothing on the page, in a field or in the status the page polls holds the topic or the token.
    const shown = await page.evaluate(() => document.documentElement.innerText + [...document.querySelectorAll('input')].map((i) => i.value).join(' '));
    assert.ok(!shown.includes(TOPIC) && !shown.includes(TOKEN), 'a secret is on the page');
    const status = await page.evaluate(async () => (await fetch('/api/admin/v1/status', { headers: { 'x-askbar-admin': '1' } })).text());
    assert.ok(!status.includes(TOPIC) && !status.includes(TOKEN), 'the status holds a secret');
    // Remove: it asks first, and the banner comes back.
    page.once('dialog', (d) => d.accept());
    await page.click('#a-remove');
    await page.waitForFunction(() => document.getElementById('a-held-topic').textContent === 'no');
    await page.waitForSelector('#banners [data-banner="alerts"]');
    assert.match(await page.innerText('#banners [data-banner="alerts"]'), /Alerts are not set up/);
  });
  await step('alerts: email is saved, tested, kept when ntfy changes, and the password is never shown again (ADR 0055)', async () => {
    const PASSWORD = 'e2e-mail-pass-' + 'v5c2n8';
    const USER = 'e2e-mail-user-' + 'h3t6w0';
    const mail = (p, method = 'GET') => new Promise((resolve, reject) => {
      const req = http.request({ host: '127.0.0.1', port: smtpSeenPort, path: p, method }, (res) => { let b = ''; res.on('data', (c) => (b += c)); res.on('end', () => resolve(b)); });
      req.on('error', reject); req.end();
    });
    await openPage('notifications');
    await page.waitForSelector('#email-form');
    assert.equal(await page.innerText('#m-held-host'), 'no');
    // ntfy is set up again first, so a test goes through both channels.
    await page.fill('#a-server', 'http://127.0.0.1:' + ntfyPort);
    await page.fill('#a-topic', 'e2e-topic-q7x9k2');
    await page.click('#alerts-form button[type=submit]');
    await page.waitForFunction(() => document.getElementById('a-held-topic').textContent === 'yes');
    // A password without TLS is refused, and the sentence names no value.
    await page.fill('#m-host', '127.0.0.1');
    await page.fill('#m-port', String(smtpPort));
    await page.selectOption('#m-tls', 'none');
    await page.fill('#m-user', USER);
    await page.fill('#m-password', PASSWORD);
    await page.fill('#m-from', 'askbar@example.com');
    await page.fill('#m-to', 'owner@example.com');
    await page.click('#email-form button[type=submit]');
    await page.waitForFunction(() => !document.getElementById('m-error').hidden);
    const refusal = await page.innerText('#m-error');
    assert.ok(!refusal.includes(PASSWORD) && !refusal.includes(USER), 'the refusal repeats a value');
    assert.equal(await page.innerText('#m-held-host'), 'no', 'nothing was saved');
    // Without the login it is saved; the fields empty and the page shows a yes or a no.
    await page.fill('#m-user', '');
    await page.fill('#m-password', '');
    await page.click('#email-form button[type=submit]');
    await page.waitForFunction(() => document.getElementById('m-held-host').textContent === 'yes');
    assert.equal(await page.innerText('#m-held-login'), 'no');
    assert.equal(await page.inputValue('#m-host'), '');
    assert.equal(await page.inputValue('#m-port'), String(smtpPort), 'the port and the mode are not secret, so they stay');
    // A test goes to both channels, and the mail arrives.
    await mail('/_seen', 'DELETE');
    await page.click('#a-test');
    await page.waitForFunction(() => /email: delivered/.test(document.getElementById('a-result').textContent) && /ntfy: delivered/.test(document.getElementById('a-result').textContent));
    const seen = JSON.parse(await mail('/_seen'));
    assert.equal(seen.length, 1);
    assert.match(seen[0].data, /Subject: Askbar test alert/);
    assert.ok(seen[0].to.includes('owner@example.com'));
    // The server refuses the recipient: the page says so in its own words.
    await mail('/_reject/550', 'POST');
    await page.click('#a-test');
    await page.waitForFunction(() => /email: failed, the server refused it/.test(document.getElementById('a-result').textContent));
    await mail('/_reject/0', 'POST');
    // Saving ntfy again does not touch email.
    await page.fill('#a-server', 'http://127.0.0.1:' + ntfyPort);
    await page.fill('#a-topic', 'e2e-topic-q7x9k2');
    await page.click('#alerts-form button[type=submit]');
    await page.waitForFunction(() => document.getElementById('a-held-topic').textContent === 'yes');
    assert.equal(await page.innerText('#m-held-host'), 'yes');
    // Nothing on the page holds the password or the user name.
    const shown = await page.evaluate(() => document.documentElement.innerText + [...document.querySelectorAll('input')].map((i) => i.value).join(' '));
    assert.ok(!shown.includes(PASSWORD) && !shown.includes(USER));
    // Remove email: it asks first, and ntfy stays.
    page.once('dialog', (d) => d.accept());
    await page.click('#m-remove');
    await page.waitForFunction(() => document.getElementById('m-held-host').textContent === 'no');
    assert.equal(await page.innerText('#a-held-topic'), 'yes');
    // Leave the settings as the next steps expect them: no channel.
    page.once('dialog', (d) => d.accept());
    await page.click('#a-remove');
    await page.waitForFunction(() => document.getElementById('a-held-topic').textContent === 'no');
  });
  await step('notifications: General holds no delivery fields, and the menu lists Notifications after Backup', async () => {
    await openPage('general');
    await page.waitForSelector('#settings-form');
    assert.equal(await page.locator('#alerts-form, #email-form, #alerts-test-form, #a-server, #m-host').count(), 0, 'General has no delivery fields');
    const menu = await page.$$eval('#nav a[data-tab]', (as) => as.map((a) => [a.dataset.tab, a.textContent.trim()]));
    const at = menu.findIndex(([slug]) => slug === 'backup');
    assert.deepEqual(menu.slice(at, at + 2).map(([slug, label]) => [slug, label]), [['backup', 'Backup'], ['notifications', 'Notifications']]);
  });
  await step('notifications: page-wide banners are greyed: critical ones checked and aria-disabled (not disabled), inert on click, Space and Enter', async () => {
    await openPage('notifications');
    await page.waitForSelector('#bn-offline');
    const boxes = ['bn-offline', 'bn-rejected', 'bn-chain', 'bn-alerts', 'bn-unsealed'];
    const read = () => page.evaluate((ids) => ids.map((id) => { const i = document.getElementById(id); return [id, i.checked, i.getAttribute('aria-disabled'), i.disabled]; }), boxes);
    const critical = ['bn-offline', 'bn-rejected', 'bn-chain'];
    const before = await read();
    for (const [id, checked, aria, disabled] of before) {
      assert.equal(aria, 'true', id + ' is aria-disabled');
      assert.equal(disabled, false, id + ' is not disabled, so keyboard focus can stay on the group');
      if (critical.includes(id)) assert.equal(checked, true, id + ' is checked (critical, always shown)');
    }
    const group = page.locator('section[aria-labelledby="banners-title"] .na');
    assert.equal(await group.getAttribute('tabindex'), '0', 'the greyed section is a Tab stop');
    assert.equal(await group.getAttribute('aria-disabled'), 'true');
    await group.focus();
    assert.equal(await page.evaluate(() => document.activeElement.classList.contains('na')), true, 'the greyed section takes focus');
    await page.keyboard.press('Enter');
    await page.keyboard.press(' ');
    await page.click('#bn-offline', { force: true });
    await page.click('label:has(#bn-alerts)', { force: true });
    assert.deepEqual(await read(), before, 'nothing changes on Enter, Space or click');
    await page.keyboard.press('Tab');
    assert.equal(await page.evaluate(() => document.activeElement.matches('section[aria-labelledby="banners-title"] input')), false, 'Tab leaves the greyed section without stopping on its checkboxes');
    const greyed = await page.$eval('section[aria-labelledby="banners-title"] .na', (n) => n.innerText);
    assert.doesNotMatch(greyed, /\d/, 'no digits in greyed text');
    const main = await page.$eval('#main', (m) => m.innerText);
    assert.doesNotMatch(main, /audit_disk|backup_missed|restore_performed|component_silent|caddy_silent|restart_loop|chain_mismatch|rejected_token/, 'no alert-kind list on the page');
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), true, 'no sideways scroll');
    const small = await page.evaluate((min) => [...document.querySelectorAll('#main button, #main a.btn, #main input:not([type=checkbox]), #main select, #main label.check')]
      .map((t) => t.getBoundingClientRect()).filter((r) => r.height > 0 && (r.height < min || r.width < min)).length, isPhone ? 44 : 24);
    assert.equal(small, 0, 'every target on the notifications page is big enough');
  });
  await step('network: hostname, token and networks are saved; the token is write-only', async () => {
    const CF = 'cf_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789';
    await openPage('network');
    await page.waitForFunction(() => /pairing keys point at/.test(document.getElementById('n-info').textContent));
    await page.check('#n-tls');
    await page.fill('#n-host', 'https://askbar.orraitu.com');
    await page.fill('#n-token', CF);
    await page.fill('#n-cidr', '192.168.1.0/24');
    assert.equal(await page.isChecked('#n-allow'), false, 'the allowlist starts off');
    await page.click('#network-form button[type=submit]');
    await page.waitForSelector('#network-error:not([hidden])');
    assert.match(await page.innerText('#network-error'), /plain DNS name/);
    await page.fill('#n-host', 'askbar.orraitu.com');
    await page.check('#n-staging');
    await page.click('#network-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    assert.match(await page.locator('#toasts .toast').last().innerText(), /Network settings saved/);
    assert.match(await page.innerText('#n-info'), /admin site is at https:\/\/askbar\.orraitu\.com\/admin/);
    assert.match(await page.innerText('#n-info'), /pairing keys point at https:\/\/askbar\.orraitu\.com\./);
    assert.equal(await page.inputValue('#n-token'), '');
    assert.match(await page.innerText('#n-token-hint'), /unchanged/, 'the stored token is said to be held, never shown');
    assert.equal((await page.content()).includes(CF), false);
    await page.reload();
    await openPage('network');
    await page.waitForFunction(() => document.getElementById('n-host').value === 'askbar.orraitu.com');
    assert.equal(await page.isChecked('#n-staging'), true);
    assert.equal(await page.isChecked('#n-allow'), false);
    assert.equal(await page.inputValue('#n-cidr'), '192.168.1.0/24', 'the addresses are kept while the allowlist is off');
    assert.equal((await page.content()).includes(CF), false);
    // Back to plain HTTP so the rest of the run keeps working the same way.
    await page.uncheck('#n-tls');
    await page.click('#network-form button[type=submit]');
    await page.waitForFunction(() => /point at http:\/\/localhost/.test(document.getElementById('n-info').textContent));
  });
  await step('network: HTTP-01 is chosen beside DNS, shows its warning, needs no token and refuses a private name (D19)', async () => {
    await openPage('network');
    await page.waitForFunction(() => /pairing keys point at/.test(document.getElementById('n-info').textContent));
    assert.equal(await page.$eval('input[name=n-challenge]:checked', (i) => i.value), 'dns', 'the DNS challenge is the default');
    assert.equal(await page.isVisible('#n-dns-box'), true);
    assert.equal(await page.isHidden('#n-http-note'), true);
    await page.check('#n-challenge-http');
    assert.equal(await page.isHidden('#n-dns-box'), true, 'no token to enter');
    assert.match(await page.innerText('#n-http-note'), /port 80/);
    assert.match(await page.innerText('#n-http-note'), /cannot work for a name that only resolves inside your network/);
    // A name that only resolves inside the network is refused, with the way out.
    await page.check('#n-tls');
    await page.fill('#n-host', 'nas.local');
    await page.click('#network-form button[type=submit]');
    await page.waitForSelector('#network-error:not([hidden])');
    assert.match(await page.innerText('#network-error'), /looks private.*DNS challenge/);
    // A public name is saved with no token entered, and the choice survives a reload.
    await page.fill('#n-host', 'askbar.orraitu.com');
    await page.click('#network-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    assert.match(await page.innerText('#n-info'), /admin site is at https:\/\/askbar\.orraitu\.com\/admin/);
    await page.reload();
    await openPage('network');
    await page.waitForFunction(() => document.getElementById('n-host').value === 'askbar.orraitu.com');
    assert.equal(await page.$eval('input[name=n-challenge]:checked', (i) => i.value), 'http');
    assert.equal(await page.isHidden('#n-dns-box'), true);
    // Back to DNS: the token box is back, and the token stored earlier is still held.
    await page.check('#n-challenge-dns');
    assert.equal(await page.isVisible('#n-dns-box'), true);
    assert.match(await page.innerText('#n-token-hint'), /unchanged/, 'the stored token is said to be held, never shown');
    // Leave things as the next steps expect: plain HTTP, the DNS challenge.
    await page.uncheck('#n-tls');
    await page.click('#network-form button[type=submit]');
    await page.waitForFunction(() => /point at http:\/\/localhost/.test(document.getElementById('n-info').textContent));
    assert.equal(await page.$eval('input[name=n-challenge]:checked', (i) => i.value), 'dns');
  });
  await step('backup downloads an encrypted bundle: a manifest, the protected key and the admin\'s and the server\'s exports, none of it readable (ADR 0013)', async () => {
    await openPage('backup');
    const [download] = await Promise.all([page.waitForEvent('download'), page.click('#backup-link')]);
    assert.match(download.suggestedFilename(), /^askbar-backup-\d+\.tar$/);
    const file = path.join(data, 'downloaded.tar');
    await download.saveAs(file);
    const bytes = fs.readFileSync(file);
    // A minimal tar reader: 512-byte headers, name at 0, octal size at 124.
    const entries = [];
    for (let at = 0; at + 512 <= bytes.length && bytes[at] !== 0;) {
      const name = bytes.subarray(at, at + 100).toString().replace(/\0.*$/, '');
      const size = parseInt(bytes.subarray(at + 124, at + 135).toString(), 8);
      entries.push([name, bytes.subarray(at + 512, at + 512 + size)]);
      at += 512 + Math.ceil(size / 512) * 512;
    }
    // The manifest and the protected key, then the admin's own export first and the server's.
    assert.deepEqual(entries.map(([n]) => n), ['manifest.json', 'backup-key.age', 'admin.age', 'server.age', 'alerter.age', 'cpa.age', 'audit.age']);
    const get = (n) => entries.find(([name]) => name === n)[1];
    const manifest = JSON.parse(get('manifest.json').toString());
    assert.equal(manifest.format, 1);
    assert.match(manifest.release, /^\d+\.\d+\.\d+/);
    assert.match(manifest.key_fingerprint, /^[0-9a-f]{4}(-[0-9a-f]{4}){3}$/);
    assert.deepEqual(manifest.parts.map((p) => p.name), ['admin.age', 'server.age', 'alerter.age', 'cpa.age', 'audit.age']);
    for (const p of manifest.parts) {
      assert.equal(p.bytes, get(p.name).length, p.name);
      assert.equal(p.sha256, require('node:crypto').createHash('sha256').update(get(p.name)).digest('hex'), p.name + ' hash');
    }
    // The key in the bundle is the protected private key (an age file made with the passphrase), and every export is an age file.
    for (const n of ['backup-key.age', 'admin.age', 'server.age', 'alerter.age', 'cpa.age', 'audit.age']) assert.equal(get(n).subarray(0, 21).toString(), 'age-encryption.org/v1', n);
    // Nothing is readable: no database header, no provider key, none of the passphrases or the password.
    for (const secret of ['SQLite format 3', LOGIN_MARKER, SECRET_KEY, BACKUP_PASS, BACKUP_PASS_2, AUDIT_PASS, PASSWORD, 'AGE-SECRET-KEY']) {
      assert.equal(bytes.includes(secret), false, 'the bundle holds ' + secret);
    }
  });
  await step('restore: this install is not empty, so the backup it just made is refused with a sentence and nothing changes (ADR 0013)', async () => {
    // A restore is for a new install: the bundle from the step above is checked, and the used install refuses it.
    // Restore is a section of Backup now (plan step 5.3); the old address redirects there, query and all.
    const old = await page.request.get(`http://127.0.0.1:${adminPort}/admin/restore/?from=old`, { maxRedirects: 0 });
    assert.equal(old.status(), 303);
    assert.equal(old.headers()['location'], '/admin/backup/?from=old');
    await openPage('backup');
    assert.equal(await page.isHidden('#restore-form'), true, 'the passphrase is not asked for before the backup is checked');
    await page.setInputFiles('#restore-file', path.join(data, 'downloaded.tar'));
    await page.click('#restore-upload-form button[type=submit]');
    await page.waitForSelector('#restore-error:not([hidden])');
    assert.match(await page.innerText('#restore-error'), /only be restored into an empty install/);
    assert.equal(await page.isHidden('#restore-form'), true);
    // The menu has no Restore item, and the Backup page's link opens the section (the first-run screen has the same link).
    assert.equal(await page.locator('#nav a[data-tab="restore"]').count(), 0, 'no Restore item in the menu');
    assert.equal(await page.locator('#tab-backup a[href="#restore-section"]').count(), 1);
  });
  await step('backup: the destination, "Back up now", the limits and the drill reminder work and show only text (ADR 0013, D27 to D29)', async () => {
    await openPage('backup');
    await page.waitForFunction(() => document.getElementById('sched-path').textContent.endsWith('/backup'));
    assert.match(await page.innerText('#sched-state'), /^writable$/);
    // The test directory is on the same disk as the admin's data, and the page says so.
    assert.match(await page.innerText('#sched-warn'), /same disk as Askbar/);
    await page.click('#sched-run');
    await page.waitForFunction(() => [...document.querySelectorAll('#toasts .toast')].some((t) => /^Backup made: askbar-backup-\d+\.tar$/.test(t.textContent)));
    await page.waitForFunction(() => document.querySelectorAll('#sched-files li').length >= 1);
    assert.match(await page.innerText('#sched-last'), /Last backup: .*askbar-backup-\d+\.tar/);
    assert.match(await page.innerText('#sched-next'), /03:30 UTC/);
    const onDisk = fs.readdirSync(path.join(data, 'backup')).filter((f) => /^askbar-backup-\d+\.tar$/.test(f));
    assert.ok(onDisk.length >= 1, 'a backup is on the volume');
    assert.deepEqual(fs.readdirSync(path.join(data, 'backup')).filter((f) => f.endsWith('.partial')), [], 'no partial file is left');
    // The limits: a value out of range is refused with a sentence; a good one is saved and read back.
    await page.evaluate(() => { document.getElementById('schedule-form').noValidate = true; });
    await page.fill('#sched-keep-files', '0');
    await page.click('#schedule-form button[type=submit]');
    await page.waitForSelector('#sched-limits-error:not([hidden])');
    assert.match(await page.innerText('#sched-limits-error'), /between 1 and 365/);
    await page.fill('#sched-keep-files', '3');
    await page.fill('#sched-keep-days', '14');
    await page.click('#schedule-form button[type=submit]');
    await page.waitForFunction(() => [...document.querySelectorAll('#toasts .toast')].some((t) => t.textContent === 'Saved.'));
    await openPage('providers');
    await openPage('backup');
    await page.waitForFunction(() => document.getElementById('sched-keep-files').value === '3');
    assert.equal(await page.inputValue('#sched-keep-days'), '14');
    // The drill reminder shows when it is due and "Drill done" clears it.
    assert.equal(await page.isHidden('#sched-reminder'), true, 'not due yet');
  });
  // ------------------------------------------------------------------ the Audit tab
  // Against the real server: a provider whose name and model are markup, one real chat through it,
  // then the Audit pages (#tab-audit-calls and its trace panel, #tab-audit-sealed, #tab-audit-routing).
  const XSS = '<img src=x onerror="window.__pwned=1">';
  // One real chat through the server, so there is something to look at; returns once the reply is done.
  async function auditChat(text, providerName) {
    const pairing = await page.evaluate(async () => {
      const H = { 'x-askbar-admin': '1', 'content-type': 'application/json' };
      const users = await (await fetch('/api/admin/v1/users', { headers: H })).json();
      const r = await fetch('/api/admin/v1/users/' + encodeURIComponent(users[0].id) + '/pairing-keys', { method: 'POST', headers: H, body: JSON.stringify({ name: 'audit test' }) });
      return (await r.json()).pairing;
    });
    const { code } = JSON.parse(Buffer.from(pairing.slice('askbar-pair:'.length), 'base64url').toString());
    const h = { 'content-type': 'application/json', 'x-askbar-client-version': '0.6.0' };
    const { token } = await (await fetch(`http://127.0.0.1:${api}/api/client/v1/pairings`, { method: 'POST', headers: h, body: JSON.stringify({ code }) })).json();
    const auth = { ...h, authorization: 'Bearer ' + token };
    // The catalog reaches the gateway a moment after the admin saves it, and the server keeps it for a few seconds.
    let model;
    for (let i = 0; i < 40 && !model; i++) {
      const list = await (await fetch(`http://127.0.0.1:${api}/api/client/v1/models`, { headers: auth })).json();
      model = (list.models || []).find((m) => m.provider_name === providerName);
      if (!model) await new Promise((r) => setTimeout(r, 500));
    }
    assert.ok(model, 'the approved model is offered to clients');
    const spaces = await (await fetch(`http://127.0.0.1:${api}/api/client/v1/spaces`, { headers: auth })).json();
    const space = spaces.find((s) => s.kind === 'personal').id;
    const body = await (await fetch(`http://127.0.0.1:${api}/api/client/v1/spaces/${space}/chat`, { method: 'POST', headers: auth, body: JSON.stringify({ model: model.id, text }) })).text();
    assert.match(body, /event: done/);
    return body;
  }
  await step('admin: Trusted Types are required, and the browser refuses every HTML sink, a policy, a script and a string of code (D17)', async () => {
    // A page of its own, so the violations this makes are not counted as the admin's own errors.
    const p = await page.context().newPage();
    await p.goto(page.url());
    await p.waitForSelector('#app:not([hidden])');
    const csp = await p.evaluate(async () => (await fetch(location.href)).headers.get('content-security-policy'));
    assert.match(csp, /require-trusted-types-for 'script'/);
    assert.match(csp, /trusted-types 'none'/);
    const r = await p.evaluate(async () => {
      const tryIt = (f) => { try { f(); return 'allowed'; } catch (e) { return e.name; } };
      return {
        innerHTML: tryIt(() => { document.body.innerHTML = '<b>x</b>'; }),
        insertAdjacentHTML: tryIt(() => document.body.insertAdjacentHTML('beforeend', '<b>x</b>')),
        policy: tryIt(() => window.trustedTypes.createPolicy('x', { createHTML: (v) => v })),
        script: tryIt(() => { const e = document.createElement('script'); e.textContent = 'window.__pwned = 1'; document.head.appendChild(e); }),
        timer: await new Promise((resolve) => {
          try { setTimeout('window.__pwned = 2', 0); } catch (e) { resolve(e.name); return; }
          setTimeout(() => resolve(window.__pwned === undefined ? 'blocked' : 'ran'), 150);
        }),
      };
    });
    assert.deepEqual(r, { innerHTML: 'TypeError', insertAdjacentHTML: 'TypeError', policy: 'TypeError', script: 'TypeError', timer: 'TypeError' });
    await p.close();
  });
  await step('admin: hostile text in user, client and provider names stays inert in every view (payload library)', async () => {
    const payloads = require('../payloads/lib.js');
    const hostile = payloads.cases().map((c) => c.input).filter((t) => t.length <= 1500);
    const origin = new URL(page.url()).origin;
    const external = [];
    const onRequest = (r) => { if (new URL(r.url()).origin !== origin) external.push(r.url()); };
    page.on('request', onRequest);
    await page.evaluate(() => { window.__pwned = undefined; });
    await page.route('**/api/admin/v1/users', (route) => route.request().method() === 'GET'
      ? route.fulfill({ json: hostile.map((name, i) => ({ id: 'h' + i, name, paused: i % 2 === 0 })) }) : route.continue());
    await page.route('**/api/admin/v1/clients', (route) => route.request().method() === 'GET'
      ? route.fulfill({ json: hostile.map((name, i) => ({ id: 'c' + i, name, user_id: 'h' + i, revoked: i % 3 === 0, created_at: 1, last_seen: 2 })) }) : route.continue());
    await page.route('**/api/admin/v1/providers', async (route) => {
      if (route.request().method() !== 'GET') return route.continue();
      const real = await (await route.fetch()).json();
      return route.fulfill({ json: hostile.map((name, i) => ({ ...real[0], id: 'p' + i, name, model: name })) });
    });
    const NASTY = '#PANEL img, #PANEL iframe, #PANEL object, #PANEL embed, #PANEL svg:not(.row-menu-button svg), #PANEL style, #PANEL base, #PANEL meta, #PANEL form, #PANEL script, #PANEL a[href^="javascript"]';
    const inert = async (panel, what) => {
      const r = await page.evaluate((nasty) => ({ nasty: document.querySelectorAll(nasty).length, pwned: window.__pwned }), NASTY.replaceAll('#PANEL', panel));
      assert.deepEqual(r, { nasty: 0, pwned: undefined }, what);
      const shown = await page.innerText(panel);
      assert.ok(shown.includes('<img src=x onerror=window.__pwned=1>'), what + ': the name is visible, as text');
      assert.ok(shown.includes('<script>window.__pwned=1</script> hello'), what + ': so is the script tag');
    };
    await openPage('people');
    await page.waitForSelector('#users [data-user="h0"]');
    await page.waitForSelector('#clients [data-client="c0"]');
    await inert('#users', 'users');
    await inert('#clients', 'clients');
    await openPage('providers');
    await page.waitForSelector('#providers tr[data-name]');
    await inert('#providers', 'providers');
    await page.unroute('**/api/admin/v1/users');
    await page.unroute('**/api/admin/v1/clients');
    await page.unroute('**/api/admin/v1/providers');
    page.off('request', onRequest);
    assert.deepEqual(external, [], 'a request left the admin site');
    await openPage('providers');
  });
  await step('backup: the new passphrase counts live, a mismatched pair blocks submit with an inline error, and nothing is sent (ADR 0013)', async () => {
    await openPage('backup');
    const min = await page.evaluate(async () => (await window.ui.ready).MIN_PASSPHRASE_CHARS);
    assert.equal(await page.innerText('#bk-pass-hint'), 'At least ' + min + ' characters');
    await page.fill('#bk-pass', 'short');
    assert.equal(await page.innerText('#bk-pass-hint'), '5 of at least ' + min + ' characters');
    assert.equal(await page.getAttribute('#bk-pass-hint', 'class'), 'hint', 'not yet long enough');
    await page.fill('#bk-pass', BACKUP_PASS_2);
    assert.equal(await page.innerText('#bk-pass-hint'), BACKUP_PASS_2.length + ' characters');
    assert.match(await page.getAttribute('#bk-pass-hint', 'class'), /length-ok/);
    // A mismatch: the error sits under the second field and the summary at the top of the form; no rotate request leaves the page.
    const sent = [];
    const seen = (req) => { if (req.url().includes('/api/admin/v1/backup/rotate-key')) sent.push(req.url()); };
    page.on('request', seen);
    await page.fill('#bk-pass2', BACKUP_PASS_2 + 'x');
    await page.check('#bk-stored');
    await page.click('#backup-key-form button[type=submit]');
    await page.waitForSelector('#bk-pass2-error:not([hidden])');
    assert.match(await page.innerText('#bk-pass2-error'), /do not match/);
    assert.equal(await page.getAttribute('#bk-pass2', 'aria-invalid'), 'true');
    assert.equal(await page.locator('#backup-key-form .error-summary').count(), 1, 'a summary at the top of the form');
    // The stored box is part of the same check.
    await page.fill('#bk-pass2', BACKUP_PASS_2);
    await page.uncheck('#bk-stored');
    await page.click('#backup-key-form button[type=submit]');
    await page.waitForSelector('#bk-stored-error:not([hidden])');
    assert.match(await page.innerText('#bk-stored-error'), /stored the passphrase/);
    page.off('request', seen);
    assert.deepEqual(sent, [], 'a refused pair sends nothing');
    // The page keeps to the frame: no sideways scroll, every target at least 44 px on a phone (24 px otherwise).
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, 'no horizontal scroll');
    const small = await page.evaluate((min) => [...document.querySelectorAll('#tab-backup button, #tab-backup a.btn, #tab-backup input:not([type=checkbox]), #tab-backup label.check')]
      .map((t) => t.getBoundingClientRect()).filter((r) => r.height > 0 && (r.height < min || r.width < min)).length, isPhone ? 44 : 24);
    assert.equal(small, 0, 'every target on this page is big enough');
    // Clear the pair again so the next step starts from empty fields.
    for (const id of ['bk-pass', 'bk-pass2']) await page.fill('#' + id, '');
    await page.uncheck('#bk-stored');
  });
  await step('backup: the key is shown by its fingerprint, and rotating asks twice, confirms, retires the old key and never shows a passphrase (ADR 0013)', async () => {
    await openPage('backup');
    await page.waitForFunction(() => /^[0-9a-f]{4}(-[0-9a-f]{4}){3}$/.test(document.getElementById('bk-fingerprint').textContent));
    const before = await page.innerText('#bk-fingerprint');
    assert.equal(await page.innerText('#bk-count'), '', 'no earlier keys');
    await page.evaluate(() => { document.getElementById('backup-key-form').noValidate = true; });
    await page.fill('#bk-pass', BACKUP_PASS_2);
    await page.fill('#bk-pass2', BACKUP_PASS_2 + 'x');
    await page.check('#bk-stored');
    await page.click('#backup-key-form button[type=submit]');
    assert.match(await page.innerText('#bk-pass2-error'), /do not match/);
    await page.fill('#bk-pass2', BACKUP_PASS_2);
    await page.fill('#bk-pass', AUDIT_PASS);
    await page.fill('#bk-pass2', AUDIT_PASS);
    page.once('dialog', (d) => d.accept());
    await page.click('#backup-key-form button[type=submit]');
    await page.waitForFunction(() => /different from the audit passphrase/.test(document.getElementById('bk-error').textContent));
    assert.equal(await page.innerText('#bk-fingerprint'), before, 'the refusal changed nothing');
    await page.fill('#bk-pass', BACKUP_PASS_2);
    await page.fill('#bk-pass2', BACKUP_PASS_2);
    page.once('dialog', (d) => d.accept());
    await page.click('#backup-key-form button[type=submit]');
    await page.waitForFunction((was) => document.getElementById('bk-fingerprint').textContent !== was && /1 earlier key retired/.test(document.getElementById('bk-count').textContent), before);
    assert.equal(await page.inputValue('#bk-pass'), '', 'the passphrase field is cleared');
    assert.equal((await page.content()).includes(BACKUP_PASS_2), false, 'the passphrase is not left in the page');
    const status = await page.evaluate(async () => (await fetch('/api/admin/v1/backup/key', { headers: { 'x-askbar-admin': '1' } })).text());
    assert.ok(!status.includes('AGE-SECRET-KEY') && !status.includes('protected'), 'the protected key is not returned');
  });
  await step('audit tabs: the four Audit pages show the menu\'s Audit links as tabs, in order, each marking its own, and the Admin actions head is greyed', async () => {
    await page.evaluate(async ({ xss, mockUrl }) => {
      const H = { 'x-askbar-admin': '1', 'content-type': 'application/json' };
      const made = await (await fetch('/api/admin/v1/providers', { method: 'POST', headers: H, body: JSON.stringify({ name: xss, type: 'openai', base_url: mockUrl, model: xss, api_key: 'sk-audit' }) })).json();
      // The model is approved like any other (ADR 0014), next to the ones already approved.
      const cat = await (await fetch('/api/admin/v1/catalog', { headers: H })).json();
      cat.models.push({ provider_id: made.id, model: xss });
      const saved = await fetch('/api/admin/v1/catalog', { method: 'PUT', headers: H, body: JSON.stringify(cat) });
      if (!saved.ok) throw new Error('the catalog was refused: ' + saved.status);
    }, { xss: XSS, mockUrl: `http://127.0.0.1:${mockPort}/v1` });
    await auditChat('audit question', XSS);
    const slugs = ['audit/calls', 'audit/admin', 'audit/sealed', 'audit/routing'];
    const menu = await page.$$eval('#nav a.sm-sub[href^="/admin/audit/"]', (a) => a.map((x) => [x.textContent.trim(), x.getAttribute('href')]));
    assert.deepEqual(menu.map(([, h]) => h), slugs.map((s) => '/admin/' + s + '/'), 'the menu lists the four Audit pages in order');
    for (const slug of slugs) {
      await openPage(slug);
      await page.waitForSelector('#tab-' + slug.replace('/', '-') + ' nav.tabs');
      const tabs = await page.$$eval('#tab-' + slug.replace('/', '-') + ' nav.tabs a.tab', (a) => a.map((x) => [x.textContent.trim(), x.getAttribute('href'), x.getAttribute('aria-current')]));
      assert.deepEqual(tabs.map(([t, h]) => [t, h]), menu, slug + ': the tabs are the menu links, in order');
      assert.deepEqual(tabs.map(([, , c]) => c), slugs.map((s) => (s === slug ? 'page' : null)), slug + ': only its own tab is current');
      assert.equal(await page.locator('#tab-' + slug.replace('/', '-') + ' nav.tabs [disabled]').count(), 0, 'no disabled attribute on the tabs');
      const tab = await page.locator('#tab-' + slug.replace('/', '-') + ' nav.tabs a.tab').first().boundingBox();
      assert.ok(tab.height >= (isPhone ? 44 : 24) - 0.5, slug + ': tab target ' + tab.height);
      // No sideways scroll at either width. A tooltip (::after) that runs past the edge counts, and is named on failure.
      const past = await page.evaluate(() => {
        const out = [];
        for (const e of document.querySelectorAll('body *')) {
          const r = e.getBoundingClientRect();
          if (r.right > window.innerWidth + 1) out.push(e.tagName + '.' + e.className + ' right ' + Math.round(r.right));
          const cs = getComputedStyle(e, '::after');
          if (cs.content !== 'none' && cs.position === 'absolute') {
            const edge = r.left + parseFloat(cs.left) + parseFloat(cs.width);
            if (edge > window.innerWidth + 1) out.push(e.tagName + '.' + e.className + ' ::after edge ' + Math.round(edge));
          }
        }
        return { scrollWidth: document.documentElement.scrollWidth, innerWidth: window.innerWidth, past: out.slice(0, 6) };
      });
      assert.equal(past.scrollWidth <= past.innerWidth, true, slug + ': no horizontal scroll ' + JSON.stringify(past));
      assert.equal(await page.locator('#tab-' + slug.replace('/', '-') + ' .sm-sub, #tab-' + slug.replace('/', '-') + ' .subnav').count(), 0, 'no in-page sub-nav');
    }
    // Admin actions: the note, and the table head greyed with aria-disabled, with no rows and no digits.
    await openPage('audit/admin');
    assert.match(await page.innerText('#tab-audit-admin .notice'), /Not available yet/);
    const heads = await page.$$eval('#admin-actions-table thead th', (h) => h.map((x) => x.querySelector('[aria-disabled="true"]')?.firstChild?.textContent || null));
    assert.deepEqual(heads, ['Time', 'Action', 'Where', 'Result'], 'every head is greyed');
    assert.equal(await page.locator('#admin-actions-table tbody tr').count(), 0, 'no sample rows');
    assert.doesNotMatch(await page.innerText('#admin-actions-table thead'), /\d/, 'no digits in greyed text');
    assert.equal(await page.locator('#tab-audit-admin [disabled]').count(), 0, 'aria-disabled, never disabled');
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, 'no horizontal scroll');
  });
  await step('audit: server strings are shown as text, never as HTML, on Routing and Sealed access', async () => {
    const nothingRan = async (where) => {
      assert.equal(await page.locator('#main img').count(), 0, where + ': no element was made from a name');
      assert.equal(await page.evaluate(() => window.__pwned), undefined, where + ': nothing ran');
    };
    await openPage('audit/routing');
    await page.waitForSelector('#audit-routing-rows tr');
    assert.match(await page.innerText('#audit-routing-table'), /<img src=x/, 'the provider name is shown as text');
    await nothingRan('routing');
    const traceId = await page.evaluate(async () => (await (await fetch('/api/admin/v1/audit/traces?limit=1', { headers: { 'x-askbar-admin': '1' } })).json()).traces[0].trace_id);
    await page.goto(base + 'audit/routing/?id=' + encodeURIComponent(traceId));
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#audit-attempt-rows tr');
    assert.match(await page.innerText('#audit-attempts'), /<img src=x/);
    await nothingRan('routing with a request');
    await openPage('audit/sealed');
    await page.waitForSelector('#audit-unseal-form:not([hidden])');
    await nothingRan('sealed access');
  });
  await step('audit sealed access: nothing is fetched until the passphrase opens a session, a wrong one is refused inline, and Seal closes it again', async () => {
    const asked = [];
    page.on('request', (r) => { if (/audit\/payloads/.test(r.url())) asked.push(r.url()); });
    await openPage('audit/sealed');
    await page.waitForSelector('#audit-unseal-form:not([hidden])');
    assert.equal(await page.getAttribute('#audit-pass', 'type'), 'password');
    assert.equal(asked.length, 0, 'nothing is fetched until a session is open');
    // A wrong passphrase opens nothing; the field is emptied.
    await page.fill('#audit-pass', 'not the passphrase at all!!');
    await page.click('#audit-unseal');
    await page.waitForFunction(() => /does not open any audit key/.test(document.getElementById('audit-pass-error').textContent));
    assert.equal(await page.inputValue('#audit-pass'), '');
    assert.equal(await page.locator('#banners [data-banner="unsealed"]').count(), 0);
    // The right one opens a session: a banner with the time left, the toggle says Unsealed, and no more prompts.
    await page.fill('#audit-pass', AUDIT_PASS);
    await page.click('#audit-unseal');
    await page.waitForSelector('#audit-open:not([hidden])');
    assert.match(await page.innerText('#audit-open-state'), /Unsealed\. It seals itself in about/);
    assert.equal(await page.isVisible('#audit-unseal-form'), false);
    assert.equal(await page.inputValue('#audit-pass'), '', 'the passphrase field is cleared after use');
    await page.waitForSelector('#banners [data-banner="unsealed"]', { timeout: 15000 });
    assert.match(await page.innerText('#banners [data-banner="unsealed"]'), /Sealed content is open/);
    assert.equal(await page.locator('#banners [data-banner="unsealed"] button').count(), 0, 'sealing lives in the status bar toggle, not the banner');
    assert.match(await page.innerText('#sb-seal-status'), /Unsealed/);
    assert.equal(await page.innerText('#seal-button'), 'Seal');
    // The unseal is an alert in the bell's Info group, not a banner (rule 39: banners are for the whole admin).
    await page.click('#bell-button');
    await page.waitForFunction(() => /Sealed audit content was unsealed/.test(document.querySelector('#bell-menu [role="group"][aria-label="Info alerts"]')?.textContent || ''), null, { timeout: 15000 });
    assert.equal(await page.locator('#banners [data-banner^="sealed-"]').count(), 0, 'no banner for a history alert');
    await page.keyboard.press('Escape');
    // The access history: the opening and the wrong guess, as text, newest first.
    await page.waitForSelector('#audit-access-rows tr');
    const results = await page.$$eval('#audit-access-rows tr td:nth-child(4)', (c) => c.map((x) => x.textContent));
    assert.deepEqual(results, ['opened', 'wrong passphrase'], 'newest first');
    assert.equal(await page.locator('#audit-access-rows td.na-cell').count(), 2 * results.length, 'Ended and How it ended are greyed');
    // Seal: the form is back, the banner goes.
    await page.click('#audit-seal');
    await page.waitForSelector('#audit-unseal-form:not([hidden])');
    await page.waitForSelector('#banners [data-banner="unsealed"]', { state: 'detached', timeout: 15000 }).catch(async () => { await page.reload(); });
    assert.equal(await page.locator('#main img').count(), 0);
    assert.equal(await page.evaluate(() => window.__pwned), undefined);
  });
  await step('audit calls: figures and table come from the real trail as text, greyed filters are inert, a row opens its trace in the docked panel, Esc and Back close it, and ?id= opens it on load', async () => {
    await openPage('audit/calls');
    await page.waitForSelector('#calls-rows tr[data-trace]');
    await page.waitForFunction(() => Number(document.getElementById('kpi-replies').textContent.replace(/,/g, '')) >= 1);
    assert.deepEqual(await page.$$eval('#calls-kpis .kpi .k', (k) => k.map((x) => x.textContent)), ['Replies', 'Errors', 'Tokens', 'Latency, median', 'Latency, 95th percentile']);
    assert.deepEqual(await page.$$eval('#calls-table th', (h) => h.map((x) => x.textContent)), ['Time', 'User', 'Provider', 'Model', 'Tokens in / out', 'Duration', 'Status']);
    assert.match(await page.innerText('#calls-table'), /<img src=x/, 'the provider and model names are visible as text');
    assert.equal(await page.locator('#tab-audit-calls img').count(), 0, 'no element was made from a name');
    assert.equal(await page.evaluate(() => window.__pwned), undefined);
    assert.equal(await page.isHidden('#trace-panel'), true, 'the panel is closed until a call is chosen');
    // Every row is one line, and a status is a word with a symbol, never colour alone.
    const tall = await page.$$eval('#calls-rows tr', (r) => r.filter((tr) => tr.getBoundingClientRect().height > tr.querySelector('.row-link').getBoundingClientRect().height + 24).length);
    assert.equal(tall, 0, 'one-line cells');
    assert.equal(await page.locator('#calls-rows tr:first-child .status .status-word').count(), 1);

    // Filters the trail cannot apply are greyed: aria-disabled, never disabled; Tab reaches them; Enter does nothing.
    const greyed = page.locator('#tab-audit-calls .calls-filters .na, #tab-audit-calls .calls-toolbar .na');
    assert.equal(await greyed.count(), 4);
    assert.deepEqual(await greyed.evaluateAll((n) => n.map((x) => [x.getAttribute('aria-disabled'), x.tabIndex, x.hasAttribute('disabled'), x.dataset.tip.startsWith('Not available yet: ')])), Array(4).fill(['true', 0, false, true]));
    assert.equal(await page.locator('#tab-audit-calls [disabled]').count(), 0, 'no disabled attribute on the page');
    const before = page.url();
    await greyed.first().focus();
    await page.keyboard.press('Enter');
    await page.keyboard.press(' ');
    assert.equal(page.url(), before, 'Enter on a greyed filter changes nothing');
    assert.equal(await page.isHidden('#trace-panel'), true);
    assert.match(await greyed.first().innerText(), /Not available yet/, 'the reason is there for a screen reader');

    // A row opens the trace in the docked panel; the address holds the call.
    const id = await page.getAttribute('#calls-rows tr[data-trace]', 'data-trace');
    await page.click(`#calls-rows tr[data-trace="${id}"] .row-link`);
    await page.waitForSelector('#trace-content:not([hidden])');
    assert.equal(await page.isVisible('#trace-panel'), true);
    assert.equal(new URL(page.url()).searchParams.get('id'), id);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'trace-panel-title', 'focus moves to the panel');
    assert.equal(await page.getAttribute(`#calls-rows tr[data-trace="${id}"] .row-link`, 'aria-current'), 'true');
    assert.equal(await page.locator('#calls-rows tr.call-sel').count(), 1, 'one selected row');
    assert.equal(await page.innerText('#trace-id'), id);
    assert.ok(await page.locator('#trace-waterfall .span-bar').count() >= 2, 'the spans are drawn');
    await page.focus('#trace-waterfall .span-bar[data-span] >> nth=-1');
    assert.match(await page.innerText('#trace-span-details'), /<img src=x/, 'the details show the model as text');
    assert.match(await page.innerText('#trace-meta'), /<img src=x/);
    assert.equal(await page.locator('#trace-panel img').count(), 0);
    assert.equal(await page.evaluate(() => window.__pwned), undefined);
    assert.deepEqual(await page.evaluate(() => (document.documentElement.scrollWidth <= window.innerWidth ? [] : [...document.querySelectorAll('body *')].filter((e) => e.getBoundingClientRect().right > window.innerWidth + 1).slice(0, 5).map((e) => e.tagName + '#' + e.id + '.' + e.className))), [], 'no sideways scroll with the panel open');
    // The old detail pane is gone from the page.
    assert.equal(await page.locator('#audit-nav, #audit-trace, #audit-waterfall').count(), 0);

    // Esc closes it, and the address forgets it.
    await page.keyboard.press('Escape');
    await page.waitForSelector('#trace-panel', { state: 'hidden' });
    await page.waitForFunction(() => !location.search.includes('id='));
    assert.equal(await page.locator('#calls-rows tr.call-sel').count(), 0, 'nothing selected any more');
    // Back closes it too.
    await page.click(`#calls-rows tr[data-trace="${id}"] .row-link`);
    await page.waitForSelector('#trace-content:not([hidden])');
    await page.goBack();
    await page.waitForSelector('#trace-panel', { state: 'hidden' });
    assert.match(page.url(), /\/admin\/audit\/calls\/$/, 'Back stays on the page');
    // The address alone opens it, and Esc closes it again.
    await page.goto(base + 'audit/calls/?id=' + encodeURIComponent(id));
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#trace-content:not([hidden])');
    assert.equal(await page.innerText('#trace-id'), id);
    await page.keyboard.press('Escape');
    await page.waitForSelector('#trace-panel', { state: 'hidden' });
    await page.waitForFunction(() => !location.search.includes('id='));
    // A call that is not in the trail says so inside the panel.
    await page.goto(base + 'audit/calls/?id=no-such-call');
    await page.waitForSelector('#trace-error:not([hidden])');
    assert.match(await page.innerText('#trace-error'), /not in the audit trail/);
    await page.keyboard.press('Escape');
    // The page keeps to the frame: targets are big enough (44 px on a phone).
    await page.goto(base + 'audit/calls/?id=' + encodeURIComponent(id));
    await page.waitForSelector('#trace-content:not([hidden])');
    const small = await page.evaluate((min) => [...document.querySelectorAll('#tab-audit-calls button, #tab-audit-calls select, #tab-audit-calls .na, #trace-panel button')]
      .map((t) => t.getBoundingClientRect()).filter((r) => r.height > 0 && (r.height < min || r.width < min)).length, isPhone ? 44 : 24);
    assert.equal(small, 0, 'every target on this page is big enough');
    // axe, with the panel open (a copy of the page in a context that lets axe in).
    const axeCtx = await browser.newContext({ viewport, bypassCSP: true, storageState: await ctx.storageState() });
    try {
      const ap = await axeCtx.newPage();
      await ap.goto(base + 'audit/calls/?id=' + encodeURIComponent(id));
      await ap.waitForSelector('#trace-content:not([hidden])');
      await ap.waitForSelector('#calls-rows tr[data-trace]');
      await ap.addScriptTag({ content: fs.readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8') });
      const found = await ap.evaluate(async () => (await window.axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } })).violations.map((v) => v.id + ': ' + v.nodes.map((n) => n.target.join(' ')).join(', ')));
      assert.deepEqual(found, [], 'axe on Model calls with the trace open');
    } finally { await axeCtx.close(); }
    await page.keyboard.press('Escape');
    await page.waitForSelector('#trace-panel', { state: 'hidden' });
  });
  await step('audit calls (canned): the figures, filters in the address, load more, and a live refresh that keeps the open panel and its row', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    const now = Math.floor(Date.now() / 1000);
    const row = (n, extra = {}) => ({ trace_id: 'T' + String(n).padStart(3, '0'), span_id: 's' + n, status: 'done', started_at: now - 60 - (200 - n), ended_at: now - 59 - (200 - n), user_id: 'user-1', client_id: 'd', tokens_in: 100, tokens_out: 50, tokens_cached: 0, error_code: null, provider_id: 'pA', model: 'model-' + n, ttft_ms: 200, latency_ms: 1500, cost: 0.0007, ...extra });
    const size = K.AUDIT_PAGE_SIZE;
    const first = Array.from({ length: size }, (_, i) => row(100 - i));
    const second = Array.from({ length: 15 }, (_, i) => row(100 - size - i));
    let head = [];
    const askedList = [];
    const askedStats = [];
    const canned = { requests: 40, by_status: { done: 36, error: 2, timed_out: 1, cancelled: 1 }, error_rate: 0.075, latency_ms: { p50: 1200, p95: 5400 }, ttft_ms: { p50: 300, p95: 900 }, tokens: { in: 12345, out: 6789, cached: 2000 }, cost: 1.2345, cost_unknown_requests: 0, by_provider: [], by_day: [] };
    await page.route('**/api/admin/v1/audit/stats*', (route) => { askedStats.push(route.request().url()); route.fulfill({ contentType: 'application/json', body: JSON.stringify(canned) }); });
    await page.route('**/api/admin/v1/audit/traces?*', (route) => {
      const q = new URL(route.request().url()).searchParams;
      askedList.push(Object.fromEntries(q));
      let traces = [...head, ...first];
      if (q.get('before') === first[first.length - 1].trace_id) traces = second;
      if (q.get('status') === 'error') traces = [row(5, { status: 'error', error_code: 'http_401', tokens_in: null, tokens_out: null, cost: null })];
      route.fulfill({ contentType: 'application/json', body: JSON.stringify({ traces }) });
    });
    await page.route('**/api/admin/v1/audit/traces/*', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ trace_id: route.request().url().split('/').pop(), cost: null, spans: [{ id: 'x-turn', parent_id: null, kind: 'turn', name: 'chat', status: 'done', started_at: now - 10, ended_at: now - 9, user_id: 'user-1', tokens_in: 100, tokens_out: 50, attrs: {} }] }) }));
    try {
      // The filters in the address apply on load.
      await page.goto(base + 'audit/calls/?status=error&provider=pA');
      await page.waitForSelector('#app:not([hidden])');
      await page.waitForSelector('#calls-rows tr[data-trace]');
      assert.equal(await page.inputValue('#calls-status'), 'error');
      assert.equal(await page.inputValue('#calls-provider'), 'pA', 'a provider named in the address filters even when it is not in the list');
      assert.deepEqual(askedList[askedList.length - 1], { status: 'error', provider: 'pA', limit: String(size) });
      assert.equal(await page.locator('#calls-rows tr').count(), 1);
      const cells = await page.$$eval('#calls-rows tr:first-child td', (c) => c.map((x) => x.textContent.trim()));
      assert.deepEqual([cells[1], cells[3], cells[4], cells[5], cells[6]], ['user-1', 'model-5', '–', '1.5 s', 'error']);
      assert.equal(await page.locator('#calls-rows tr.call-err').count(), 1, 'a failed call is tinted');
      assert.equal(await page.isHidden('#calls-more'), true, 'a short page means nothing more');
      // The figures come from the stats read for the range.
      assert.deepEqual(await page.$$eval('#calls-kpis .kpi .v', (v) => v.map((x) => x.textContent)), ['40', '3', '19,134', '1.2 s', '5.4 s']);
      assert.equal(await page.locator('#kpi-errors.bad').count(), 1);
      const since = (u) => Number(new URL(u).searchParams.get('since'));
      assert.ok(Math.abs(Math.floor(Date.now() / 1000) - since(askedStats.at(-1)) - 86400) < 120, 'the first range is the last 24 hours');

      // A filter changed on the page changes the address, and starts again from the newest call.
      await page.selectOption('#calls-status', '');
      await page.waitForFunction((n) => document.querySelectorAll('#calls-rows tr').length === n, size);
      assert.equal(new URL(page.url()).searchParams.has('status'), false);
      assert.equal(new URL(page.url()).searchParams.get('provider'), 'pA', 'the other filter stays');
      assert.deepEqual(askedList[askedList.length - 1], { provider: 'pA', limit: String(size) });
      assert.equal(await page.innerText('#calls-count'), 'Showing ' + size + ' calls');
      await page.selectOption('#calls-status', 'timed_out');
      assert.equal(new URL(page.url()).searchParams.get('status'), 'timed_out', 'the address holds the auditor\'s code');
      await page.selectOption('#calls-status', '');
      await page.waitForFunction((n) => document.querySelectorAll('#calls-rows tr').length === n, size);

      // Load more asks for the page after the last row shown and appends; a short page ends it.
      await page.click('#calls-more');
      await page.waitForFunction((n) => document.querySelectorAll('#calls-rows tr').length === n, size + 15);
      assert.equal(askedList[askedList.length - 1].before, first[first.length - 1].trace_id);
      assert.equal(await page.isHidden('#calls-more'), true);
      assert.equal(await page.innerText('#calls-count'), 'Showing ' + (size + 15) + ' calls');

      // A live refresh keeps the open panel, its address and its row, and adds the new call.
      await page.click('#calls-rows tr[data-trace="T099"] .row-link');
      await page.waitForSelector('#trace-content:not([hidden])');
      await page.evaluate(() => { window.__keptRow = document.querySelector('#calls-rows tr[data-trace="T099"]'); });
      head = [row(150)];
      await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
      await page.waitForSelector('#calls-rows tr[data-trace="T150"]');
      assert.equal(await page.locator('#calls-rows tr').count(), size + 16, 'the loaded pages are kept under the new call');
      assert.equal(await page.locator('#calls-rows tr.row-new').count(), 1, 'the new call is marked');
      assert.equal(await page.isVisible('#trace-panel'), true, 'the panel stays open');
      assert.equal(new URL(page.url()).searchParams.get('id'), 'T099');
      assert.equal(await page.locator('#calls-rows tr.call-sel').getAttribute('data-trace'), 'T099', 'the selected row stays selected');
      assert.equal(await page.evaluate(() => window.__keptRow === document.querySelector('#calls-rows tr[data-trace="T099"]')), true, 'and is the same row, not a rebuilt one');
      // Another row while the panel is open replaces the address rather than adding to it.
      // (On a phone the panel covers the list, so another row is chosen only after closing it.)
      if (!isPhone) {
        const length = await page.evaluate(() => history.length);
        await page.click('#calls-rows tr[data-trace="T098"] .row-link');
        await page.waitForFunction(() => new URL(location.href).searchParams.get('id') === 'T098');
        assert.equal(await page.evaluate(() => history.length), length);
        await page.waitForFunction(() => document.getElementById('trace-id').textContent === 'T098');
        assert.equal(await page.locator('#calls-rows tr.call-sel').count(), 1);
        assert.equal(new URL(page.url()).searchParams.get('provider'), 'pA', 'the filter stays in the address with the trace');
      }
      await page.keyboard.press('Escape');
      await page.waitForSelector('#trace-panel', { state: 'hidden' });
      assert.equal(new URL(page.url()).searchParams.get('provider'), 'pA');
      assert.equal(new URL(page.url()).searchParams.has('id'), false);

      // The range asks for its own window.
      head = [];
      await page.selectOption('#calls-window', '7');
      await page.waitForFunction(() => document.querySelectorAll('#calls-rows tr').length > 0);
      assert.ok(Math.abs(Math.floor(Date.now() / 1000) - since(askedStats.at(-1)) - 7 * 86400) < 120, 'then the last 7 days');
      // An empty answer says so instead of showing an empty table.
      await page.unroute('**/api/admin/v1/audit/traces?*');
      await page.route('**/api/admin/v1/audit/traces?*', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ traces: [] }) }));
      await page.selectOption('#calls-window', '30');
      await page.waitForSelector('#calls-empty:not([hidden])');
      assert.match(await page.innerText('#calls-empty'), /No model calls match these filters/);
      assert.equal(await page.isHidden('#calls-table'), true);
    } finally {
      await page.unroute('**/api/admin/v1/audit/stats*');
      await page.unroute('**/api/admin/v1/audit/traces?*');
      await page.unroute('**/api/admin/v1/audit/traces/*');
    }
  });
  await step('audit calls: sealed content stays sealed in the trace panel until the status bar unseals it, then View shows it as text, and sealing wipes it', async () => {
    const asked = [];
    page.on('request', (r) => { if (/audit\/payloads/.test(r.url())) asked.push(r.url()); });
    await openPage('audit/calls');
    await page.waitForSelector('#calls-rows tr[data-trace]');
    await page.click('#calls-rows tr[data-trace] .row-link');
    await page.waitForSelector('#trace-sealed .sealed-row');
    assert.match(await page.innerText('#trace-sealed-note'), /Sealed\. Opening needs the audit passphrase/);
    assert.equal(await page.locator('#trace-panel input[type="password"]').count(), 0, 'the passphrase is asked for in the status bar box, not here');
    assert.equal(asked.length, 0, 'nothing is fetched until a session is open');
    // View while sealed sends you to the Unseal box: still nothing fetched.
    await page.click('#trace-sealed .view-btn >> nth=0');
    await page.waitForSelector('#unseal-box[open]');
    assert.match(await page.innerText('#trace-sealed-error'), /Enter the audit passphrase/);
    assert.equal(asked.length, 0);
    // The right passphrase opens the session, and the item the person asked for opens with it.
    await page.fill('#unseal-pass', AUDIT_PASS);
    await page.click('#unseal-go');
    await page.waitForSelector('#unseal-box', { state: 'hidden' });
    await page.waitForFunction(() => /Anything else\?/.test(document.getElementById('trace-viewed').textContent));
    assert.equal(await page.isHidden('#trace-sealed-error'), true);
    assert.match(await page.innerText('#trace-sealed-note'), /Unsealed\. Each item you view is recorded/);
    assert.ok(asked.length >= 1);
    assert.equal(await page.locator('#trace-viewed b, #trace-viewed img').count(), 0, 'shown as text');
    assert.equal(await page.evaluate(() => JSON.stringify([...Object.entries(localStorage), ...Object.entries(sessionStorage)]).includes('passphrase')), false);
    // Closing the panel wipes what was opened; opening again asks nothing more while the session lasts.
    await page.keyboard.press('Escape');
    await page.waitForSelector('#trace-panel', { state: 'hidden' });
    assert.equal(await page.locator('#trace-viewed *').count(), 0, 'closing the panel wipes it');
    await page.click('#calls-rows tr[data-trace] .row-link');
    await page.waitForSelector('#trace-sealed .sealed-row');
    await page.click('#trace-sealed .view-btn >> nth=0');
    await page.waitForFunction(() => /Anything else\?/.test(document.getElementById('trace-viewed').textContent));
    // Seal from the status bar: the text is gone from the page.
    await page.click('#seal-button');
    await page.waitForFunction(() => /^Sealed$/.test(document.getElementById('sb-seal-status').textContent));
    await page.waitForFunction(() => document.getElementById('trace-viewed').children.length === 0);
    assert.equal((await page.content()).includes('Anything else?'), false, 'the opened text is gone from the page once sealed');
    assert.match(await page.innerText('#trace-sealed-note'), /^Sealed\./);
    assert.equal(await page.locator('#tab-audit-calls img').count(), 0);
    await page.keyboard.press('Escape');
    await page.waitForSelector('#banners [data-banner="unsealed"]', { state: 'detached', timeout: 15000 }).catch(async () => { await page.reload(); });
  });
  await step('seal toggle: unseal from another page, a wrong passphrase is an inline error, the countdown is the server\'s, Seal closes it', async () => {
    await openPage('network');
    await page.waitForSelector('#seal-button:not([hidden])');
    assert.match(await page.innerText('#sb-seal-status'), /^Sealed$/, 'the state is a word');
    assert.equal(await page.locator('#sb-seal-status .status-sym').count(), 1, 'and a symbol');
    assert.equal(await page.innerText('#seal-button'), 'Unseal');
    assert.equal(await page.getAttribute('#seal-button', 'aria-label'), 'Unseal content', 'the toggle has a name');
    const size = await page.locator('#seal-button').boundingBox();
    assert.ok(size.height >= (isPhone ? 44 : 24) - 0.5, 'target size ' + size.height);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, 'no horizontal scroll');
    await page.click('#seal-button');
    await page.waitForSelector('#unseal-box[open]');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'unseal-pass', 'focus starts in the passphrase field');
    // A wrong passphrase: the error is inline in the box, and no toast.
    await page.fill('#unseal-pass', 'not the passphrase at all!!');
    await page.click('#unseal-go');
    await page.waitForFunction(() => /does not open any audit key/.test(document.getElementById('unseal-pass-error').textContent));
    assert.equal(await page.isVisible('#unseal-pass-error'), true);
    assert.equal(await page.inputValue('#unseal-pass'), '', 'the field is emptied');
    assert.equal(await page.locator('#toasts .toast').count(), 0, 'not a toast');
    assert.equal(await page.evaluate(() => document.getElementById('unseal-box').open), true, 'the box stays open');
    // The right one: the box closes and the toggle shows the state and the server's time left.
    await page.fill('#unseal-pass', AUDIT_PASS);
    await page.click('#unseal-go');
    await page.waitForSelector('#unseal-box', { state: 'hidden' });
    await page.waitForFunction(() => /^Unsealed/.test(document.getElementById('sb-seal-status').textContent));
    const status = await page.evaluate(async () => (await (await fetch('/api/admin/v1/status', { headers: { 'X-Askbar-Admin': '1' } })).json()).sealed_content);
    const serverMins = Math.max(1, Math.ceil(Math.min(status.idle_left_secs, status.total_left_secs) / 60));
    assert.match(await page.innerText('#sb-seal-status'), new RegExp('^Unsealed \u00b7 ' + serverMins + ' min left$'));
    assert.equal(await page.innerText('#seal-button'), 'Seal');
    assert.equal(await page.getAttribute('#seal-button', 'aria-label'), 'Seal content now');
    // The Audit content view sees the session the toggle opened, without asking for the passphrase again.
    await page.goto(base + 'audit/sealed/');
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#audit-open:not([hidden])');
    assert.equal(await page.isVisible('#audit-unseal-form'), false);
    // Seal from the toggle closes the session everywhere.
    await page.click('#seal-button');
    await page.waitForFunction(() => /^Sealed$/.test(document.getElementById('sb-seal-status').textContent));
    await page.waitForSelector('#audit-unseal-form:not([hidden])');
    assert.equal(await page.innerText('#seal-button'), 'Unseal');
    await page.waitForSelector('#banners [data-banner="unsealed"]', { state: 'detached', timeout: 15000 });
  });
  await step('alerts bell: the badge counts critical plus warning, the menu groups them with links, a new critical alert pops up once, arrows and Esc work', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    const wait = K.STATUS_EVERY_SECS * 2 * 1000 + 3000;
    const clean = { server_online: true, token_rejected: false, audit_chain: { state: 'ok' }, alerts: 'ok', sealed_content: { open: false, alerts: [] } };
    let fake = clean;
    await page.route('**/api/admin/v1/status', async (route) => {
      const real = await (await route.fetch()).json();
      await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ ...real, ...fake }) });
    });
    try {
      await page.goto(base + 'network/');
      await page.evaluate(() => { try { sessionStorage.removeItem('askbar-admin.alert-pops'); } catch (_) { /* none */ } });
      await page.reload();
      await page.waitForSelector('#app:not([hidden])');
      const label = () => page.getAttribute('#bell-button', 'aria-label');
      await page.waitForFunction(() => document.getElementById('bell-button').getAttribute('aria-label') === 'Alerts: none', null, { timeout: wait });
      assert.equal(await page.isHidden('#bell-badge'), true, 'no badge at 0');
      assert.equal(await page.locator('#alert-pops .alert-pop').count(), 0);
      const box = await page.locator('#bell-button').boundingBox();
      assert.ok(box.width >= (isPhone ? 44 : 24) - 0.5 && box.height >= (isPhone ? 44 : 24) - 0.5, 'bell target ' + box.width + 'x' + box.height);
      await page.click('#bell-button');
      assert.equal(await page.getAttribute('#bell-button', 'aria-expanded'), 'true');
      assert.deepEqual(await page.$$eval('#bell-menu [role="menuitem"]', (a) => a.map((x) => [x.textContent.replace(/\s+/g, ' ').trim(), x.getAttribute('aria-disabled')])),
        [['No alerts', 'true'], ['See all alerts', null]], 'empty state, and a live link to the Alerts page (step 4.4: it was greyed until then)');
      await page.keyboard.press('Escape');

      // One critical (chain), two warnings (delivery not set up, sealed open), two infos.
      const now = Math.floor(Date.now() / 1000);
      fake = { ...clean, alerts: 'not_set_up', audit_chain: { state: 'edited', detail: 'An audit record was edited.' },
        sealed_content: { open: true, idle_left_secs: 600, total_left_secs: 900, alerts: [
          { kind: 'unsealed', at: now - 120, text: 'Sealed content was opened.' }, { kind: 'retention_changed', at: now - 60, text: 'Retention was changed.' },
          { kind: 'chain_mismatch', at: now - 30, text: 'Not counted twice.' }] } };
      await page.waitForFunction(() => document.getElementById('bell-button').getAttribute('aria-label') === 'Alerts: 1 critical, 2 warning, 2 info', null, { timeout: wait });
      assert.equal(await page.innerText('#bell-badge'), '3', 'critical plus warning, not info');
      assert.match(await page.getAttribute('#bell-badge', 'class'), /bell-badge-critical/);
      assert.equal(await page.getAttribute('#bell-button', 'data-tip'), await label(), 'the tooltip says the same');
      // The pop-up: once, with the text, a link and a close button; focus stays where it was and it was announced assertively.
      await page.waitForSelector('#alert-pops .alert-pop');
      assert.equal(await page.locator('#alert-pops .alert-pop').count(), 1);
      assert.match(await page.innerText('#alert-pops .alert-pop'), /An audit record was edited\./);
      assert.equal(await page.getAttribute('#alert-pops .alert-pop a', 'href'), '/admin/audit/calls/');
      assert.equal(await page.evaluate(() => !!document.activeElement.closest('#alert-pops')), false, 'the pop-up does not take focus');
      assert.match(await page.textContent('#announcer-urgent'), /Critical alert: An audit record was edited\./);
      assert.equal(await page.locator('#toasts .toast').count(), 0, 'not a toast');
      // The menu: groups in order, each item a link to where it is fixed.
      await page.click('#bell-button');
      assert.deepEqual(await page.$$eval('#bell-menu [role="group"]', (g) => g.map((x) => x.getAttribute('aria-label'))), ['Critical alerts', 'Warning alerts', 'Info alerts']);
      assert.deepEqual(await page.$$eval('#bell-menu a[role="menuitem"]', (a) => a.map((x) => x.getAttribute('href'))),
        ['/admin/audit/calls/', '/admin/notifications/#alerts-form', '/admin/audit/sealed/', '/admin/audit/admin/', '/admin/audit/sealed/', '/admin/alerts/']);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, 'no horizontal scroll');
      const fit = await page.evaluate(() => { const r = document.getElementById('bell-menu').getBoundingClientRect(); return r.left >= 0 && r.right <= window.innerWidth; });
      assert.equal(fit, true, 'the menu fits the screen');
      // axe, with the menu and the pop-up showing (a copy of this page in a context that lets axe in).
      const axeCtx = await browser.newContext({ viewport, bypassCSP: true, storageState: await ctx.storageState() });
      try {
        const ap = await axeCtx.newPage();
        await ap.route('**/api/admin/v1/status', async (route) => {
          const real = await (await route.fetch()).json();
          await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ ...real, ...fake }) });
        });
        await ap.goto(base + 'network/');
        await ap.waitForSelector('#alert-pops .alert-pop', { timeout: wait });
        await ap.click('#bell-button');
        await ap.addScriptTag({ content: fs.readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8') });
        const found = await ap.evaluate(async () => (await window.axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } })).violations.map((v) => v.id + ': ' + v.nodes.map((n) => n.target.join(' ')).join(', ')));
        assert.deepEqual(found, [], 'axe on the open menu and pop-up');
      } finally { await axeCtx.close(); }
      await page.keyboard.press('Escape');
      // Keyboard: ArrowDown on the button opens on the first item, arrows move, End reaches the last, Esc returns to the button.
      await page.focus('#bell-button');
      await page.keyboard.press('ArrowDown');
      assert.equal(await page.evaluate(() => document.activeElement.getAttribute('href')), '/admin/audit/calls/');
      await page.keyboard.press('ArrowDown');
      assert.equal(await page.evaluate(() => document.activeElement.getAttribute('href')), '/admin/notifications/#alerts-form');
      await page.keyboard.press('End');
      assert.equal(await page.evaluate(() => document.activeElement.textContent.startsWith('See all alerts')), true);
      assert.equal(await page.evaluate(() => document.activeElement.getAttribute('href')), '/admin/alerts/', 'the last item is a link to the Alerts page');
      assert.equal(await page.evaluate(() => document.activeElement.hasAttribute('aria-disabled')), false, 'and it is no longer greyed');
      await page.keyboard.press('Escape');
      assert.equal(await page.evaluate(() => document.activeElement.id), 'bell-button');
      assert.equal(await page.getAttribute('#bell-button', 'aria-expanded'), 'false');
      // Once only: closed, it does not come back on the next poll, nor after a reload in the same tab.
      await page.click('#alert-pops .alert-pop button');
      assert.equal(await page.locator('#alert-pops .alert-pop').count(), 0);
      await page.waitForTimeout(K.STATUS_EVERY_SECS * 1000 + 1500);
      assert.equal(await page.locator('#alert-pops .alert-pop').count(), 0, 'not again');
      await page.reload();
      await page.waitForFunction(() => document.getElementById('bell-button').getAttribute('aria-label') === 'Alerts: 1 critical, 2 warning, 2 info', null, { timeout: wait });
      assert.equal(await page.locator('#alert-pops .alert-pop').count(), 0, 'not after a reload either');
      // The condition ends: the count goes, and a later critical alert pops up again.
      fake = clean;
      await page.waitForFunction(() => document.getElementById('bell-button').getAttribute('aria-label') === 'Alerts: none', null, { timeout: wait });
      assert.equal(await page.isHidden('#bell-badge'), true);
      fake = { ...clean, token_rejected: true };
      await page.waitForSelector('#alert-pops .alert-pop', { timeout: wait });
      assert.equal(await label(), 'Alerts: 1 critical');
      fake = clean;
      await page.waitForSelector('#alert-pops .alert-pop', { state: 'detached', timeout: wait });
    } finally {
      await page.unroute('**/api/admin/v1/status');
    }
  });
  await step('alerts: Critical, Warning and Info in order with the bell\'s counts, columns that line up, Info dismissed at once and kept on reload, none for the others, "ongoing" without a time, the empty state', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    const wait = K.STATUS_EVERY_SECS * 2 * 1000 + 3000;
    const clean = { server_online: true, token_rejected: false, audit_chain: { state: 'ok' }, alerts: 'ok', sealed_content: { open: false, alerts: [] } };
    const now = Math.floor(Date.now() / 1000);
    let fake = clean;
    const fakeStatus = async (p) => p.route('**/api/admin/v1/status', async (route) => {
      const real = await (await route.fetch()).json();
      await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ ...real, ...fake }) });
    });
    await fakeStatus(page);
    try {
      await page.goto(base + 'alerts/');
      await page.evaluate(() => { try { localStorage.removeItem('askbar-admin.dismissed-alerts'); } catch (_) { /* none */ } });
      await page.reload();
      await page.waitForSelector('#app:not([hidden])');
      assert.equal(await page.getAttribute('#nav a[data-tab="alerts"]', 'aria-current'), 'page');
      assert.equal(await page.locator('#app .footer, #tab-alerts .ap-foot').count(), 0, 'no footer text');
      await page.waitForSelector('#alerts-empty .empty-state', { timeout: wait });
      assert.equal(await page.innerText('#alerts-empty'), 'No alerts');
      assert.equal(await page.locator('#alerts-body .alerts-group:not([hidden])').count(), 0, 'no group when there is nothing');

      // One critical (chain), two warnings (delivery not set up, sealed open), two infos.
      fake = { ...clean, alerts: 'not_set_up', audit_chain: { state: 'edited', detail: 'An audit record was edited.' },
        sealed_content: { open: true, idle_left_secs: 600, total_left_secs: 900, alerts: [
          { kind: 'unsealed', at: now - 120, text: 'Sealed content was opened.' }, { kind: 'retention_changed', at: now - 60, text: 'Retention was changed.' }] } };
      await page.waitForSelector('#alerts-info .alerts-row', { timeout: wait });
      assert.equal(await page.isHidden('#alerts-empty'), true);
      assert.deepEqual(await page.$$eval('#alerts-body .alerts-group:not([hidden]) > h3', (h) => h.map((x) => x.textContent.replace(/\s+/g, ' ').trim())), ['Critical · 1', 'Warning · 2', 'Info · 2'], 'groups in order, with counts');
      assert.equal(await page.innerText('#alerts-sum'), '1 critical, 2 warnings, 2 info');
      // The counts are the bell's, by construction: the badge is critical plus warning, the menu has the same rows.
      const rows = await page.$$eval('#alerts-body .alerts-row', (r) => r.map((x) => ({
        cls: x.className.replace('alerts-row ', ''), word: x.querySelector('.status-word').textContent, sym: !!x.querySelector('.status-sym'),
        time: x.querySelector('.alerts-time').textContent, fix: x.querySelector('a.alerts-fix').getAttribute('href'), dismiss: x.querySelectorAll('button').length,
      })));
      assert.deepEqual(rows.map((r) => r.word), ['Critical', 'Warning', 'Warning', 'Info', 'Info']);
      assert.equal(rows.every((r) => r.sym), true, 'a symbol with every word (rule 37)');
      assert.deepEqual(rows.map((r) => r.dismiss), [0, 0, 0, 1, 1], 'only Info can be dismissed');
      assert.deepEqual(rows.map((r) => r.time === 'ongoing'), [true, true, true, false, false], 'a condition says ongoing; an entry has its time');
      assert.equal(await page.locator('#alerts-info time[title]').count(), 2, 'the full date is on hover');
      await page.waitForFunction(() => document.getElementById('bell-badge').textContent === '3');
      assert.equal(await page.locator('#alerts-critical .alerts-row, #alerts-warning .alerts-row').count(), Number(await page.innerText('#bell-badge')), 'critical plus warning match the badge');
      assert.equal(await page.getAttribute('#bell-button', 'aria-label'), 'Alerts: 1 critical, 2 warning, 2 info');
      // Columns line up across groups: the same x for the text, time, link and action cells.
      const xs = await page.$$eval('#alerts-body .alerts-row', (r) => r.map((x) => [...x.children].map((c) => Math.round(c.getBoundingClientRect().left))));
      if (!isPhone) {
        assert.equal(new Set(xs.map((r) => r.join())).size, 1, 'every row has its cells at the same x, in every group: ' + JSON.stringify(xs));
      } else {
        const stacked = await page.$$eval('#alerts-body .alerts-row', (r) => r.map((x) => new Set([...x.children].map((c) => Math.round(c.getBoundingClientRect().left))).size));
        assert.deepEqual(stacked, stacked.map(() => 1), 'at phone width every cell stacks at the same left edge');
      }
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, 'no horizontal scroll');
      const min = isPhone ? 44 : 24;
      for (const [w, h] of await page.$$eval('#alerts-body a, #alerts-body button', (a) => a.map((x) => { const r = x.getBoundingClientRect(); return [r.width, r.height]; }))) {
        assert.ok(w >= min - 0.5 && h >= min - 0.5, 'target ' + w + 'x' + h);
      }
      // axe with all three groups showing.
      const axeCtx = await browser.newContext({ viewport, bypassCSP: true, storageState: await ctx.storageState() });
      try {
        const ap = await axeCtx.newPage();
        await fakeStatus(ap);
        await ap.goto(base + 'alerts/');
        await ap.waitForSelector('#alerts-info .alerts-row', { timeout: wait });
        await ap.addScriptTag({ content: fs.readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8') });
        const found = await ap.evaluate(async () => (await window.axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } })).violations.map((v) => v.id + ': ' + v.nodes.map((n) => n.target.join(' ')).join(', ')));
        assert.deepEqual(found, [], 'axe on the Alerts page');
      } finally { await axeCtx.close(); }

      // Dismiss the first Info row (the newest): it goes at once, from the bell too, and does not come back on a reload.
      const first = await page.locator('#alerts-info .alerts-row >> nth=0').innerText();
      assert.match(first, /Retention was changed\./, 'newest first');
      await page.click('#alerts-info .alerts-row >> nth=0 >> button');
      assert.equal(await page.locator('#alerts-info .alerts-row').count(), 1, 'removed at once');
      assert.equal((await page.textContent('#alerts-info > h3')).replace(/\s+/g, ' ').trim(), 'Info · 1');
      assert.equal(await page.getAttribute('#bell-button', 'aria-label'), 'Alerts: 1 critical, 2 warning, 1 info', 'the bell counts it too');
      assert.equal(await page.evaluate(() => !!document.activeElement.closest('#alerts-info')), true, 'focus moves to the next Info row');
      assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem('askbar-admin.dismissed-alerts')).length), 1);
      await page.reload();
      await page.waitForSelector('#alerts-info .alerts-row', { timeout: wait });
      assert.equal(await page.locator('#alerts-info .alerts-row').count(), 1, 'it does not return on a reload');
      assert.doesNotMatch(await page.innerText('#alerts-info'), /Retention was changed/);
      await page.click('#bell-button');
      assert.doesNotMatch(await page.innerText('#bell-menu [aria-label="Info alerts"]'), /Retention was changed/, 'and it is gone from the bell');
      await page.keyboard.press('Escape');
      // New alerts arrive by themselves (ui.live), highlighted and announced; the conditions end and the rows go.
      fake = { ...fake, sealed_content: { ...fake.sealed_content, alerts: [...fake.sealed_content.alerts, { kind: 'backup_failed', at: now, text: 'The backup failed.' }] } };
      await page.waitForFunction(() => document.querySelectorAll('#alerts-info .alerts-row').length === 2, null, { timeout: wait });
      assert.equal(await page.getAttribute('#alerts-info .alerts-row >> nth=0', 'data-key'), 'alert-backup_failed-' + now);
      assert.equal(await page.locator('#alerts-info .alerts-row[data-key="alert-backup_failed-' + now + '"] a').getAttribute('href'), '/admin/backup/');
      fake = clean;
      await page.waitForSelector('#alerts-empty .empty-state', { timeout: wait });
      assert.equal(await page.locator('#alerts-body .alerts-group:not([hidden])').count(), 0);
    } finally {
      await page.unroute('**/api/admin/v1/status');
      await page.evaluate(() => { try { localStorage.removeItem('askbar-admin.dismissed-alerts'); } catch (_) { /* none */ } });
    }
  });
  await step('home: quick actions (one filled primary), Needs attention from the bell items plus setup gaps, each linking to its fix, and the empty state', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    const wait = K.LIVE_EVERY_SECS * 2 * 1000 + 3000;
    const clean = { server_online: true, token_rejected: false, audit_chain: { state: 'ok' }, alerts: 'ok', sealed_content: { open: false, alerts: [] } };
    const healthy = {
      providers: [{ id: 'p1', name: 'Main' }], catalog: { models: [{ provider_id: 'p1', model: 'm' }], default: 'p1/m' }, users: [{ id: 'u1', name: 'Ann' }],
      clients: [{ id: 'c1' }], 'backup/schedule': { enabled: true, destination: { path: '/b', writable: true, problem: null, same_disk: false, files: [] }, last_success_at: 100, drill_due: false },
      network: { tls_enabled: false, hostname: '' }, // plain HTTP with no host name is fine: no row
    };
    let fake = clean;
    let setup = healthy;
    await page.route('**/api/admin/v1/status', async (route) => {
      const real = await (await route.fetch()).json();
      await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ ...real, ...fake }) });
    });
    await page.route(/\/api\/admin\/v1\/(providers|catalog|users|clients|backup\/schedule|network)$/, async (route) => {
      if (route.request().method() !== 'GET') { await route.continue(); return; }
      const key = new URL(route.request().url()).pathname.replace('/api/admin/v1/', '');
      await route.fulfill({ contentType: 'application/json', body: JSON.stringify(setup[key]) });
    });
    try {
      await page.goto(base + 'home/');
      await page.evaluate(() => { try { sessionStorage.removeItem('askbar-admin.alert-pops'); } catch (_) { /* none */ } });
      await page.reload();
      await page.waitForSelector('#app:not([hidden])');
      // Nothing wrong: the empty state, and no rows.
      await page.waitForSelector('#attention-empty:not([hidden]) .empty-state', { timeout: wait });
      assert.equal((await page.innerText('#attention-empty')).trim(), 'Nothing needs attention');
      assert.equal(await page.locator('#attention-list .arow').count(), 0);
      assert.equal(await page.innerText('#attention-count'), '0 items');
      // Quick actions: exactly one filled primary on the page; the rest are secondary; links go where they say.
      assert.equal(await page.locator('.btn-primary:visible, button.primary:visible').count(), 1, 'one filled primary');
      assert.equal(await page.locator('#quick-actions .btn-primary').count(), 1);
      assert.deepEqual(await page.$$eval('#quick-actions a', (a) => a.map((x) => [x.textContent.trim(), x.getAttribute('href')])),
        [['Pair a client', '/admin/people/#pair-open'], ['Add provider', '/admin/providers/#add-provider'], ['Open audit', '/admin/audit/calls/']]);
      for (const sel of ['#quick-actions .btn']) {
        for (const box of await page.$$eval(sel, (a) => a.map((x) => { const r = x.getBoundingClientRect(); return [r.width, r.height]; }))) {
          assert.ok(box[0] >= (isPhone ? 44 : 24) - 0.5 && box[1] >= (isPhone ? 44 : 24) - 0.5, 'quick action target ' + box);
        }
      }
      // Hairlines, not boxes (rule 7).
      const look = await page.evaluate(() => {
        const cs = (s) => getComputedStyle(document.querySelector(s));
        const sec = cs('#attention');
        return { box: ['Top', 'Right', 'Bottom', 'Left'].some((d) => parseFloat(sec['border' + d + 'Width']) > 0) || sec.boxShadow !== 'none' || sec.backgroundColor !== 'rgba(0, 0, 0, 0)', rule: parseFloat(cs('.home-sec-head').borderBottomWidth) };
      });
      assert.equal(look.box, false, 'the section is not a box');
      assert.equal(look.rule, 1, 'a hairline under the heading');

      // Back up now: success is a toast, a refusal is an inline line, and the button is busy meanwhile.
      await page.route('**/api/admin/v1/backup/run', (route) => route.fulfill({ status: 409, contentType: 'application/problem+json', body: JSON.stringify({ error: 'backup_failed', detail: 'The destination is full.' }) }));
      await page.click('#home-backup');
      await page.waitForSelector('#home-error:not([hidden])');
      assert.equal(await page.innerText('#home-error'), 'The destination is full.');
      assert.equal(await page.locator('#toasts .toast').count(), 0, 'an error is not a toast');
      await page.unroute('**/api/admin/v1/backup/run');
      await page.route('**/api/admin/v1/backup/run', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ file: 'askbar-1.tar' }) }));
      await page.click('#home-backup');
      await page.waitForSelector('#toasts .toast');
      assert.match(await page.innerText('#toasts .toast'), /Backup made: askbar-1\.tar/);
      assert.equal(await page.isHidden('#home-error'), true, 'the error line clears');
      assert.equal(await page.innerText('#home-backup'), 'Back up now', 'the button is back');
      await page.unroute('**/api/admin/v1/backup/run');

      // One critical (chain), two alert warnings (delivery not set up, sealed open), plus setup gaps: no provider, no backup key, and HTTPS on with no host name (HTTPS off with no host name, as in the healthy set, gives no row).
      fake = { ...clean, alerts: 'not_set_up', audit_chain: { state: 'edited', detail: 'An audit record was edited.' },
        sealed_content: { open: true, idle_left_secs: 600, total_left_secs: 900, alerts: [{ kind: 'retention_changed', at: Math.floor(Date.now() / 1000), text: 'Retention was changed.' }] } };
      setup = { ...healthy, providers: [], 'backup/schedule': { ...healthy['backup/schedule'], enabled: false }, network: { tls_enabled: true, hostname: '' } };
      await page.waitForFunction(() => document.querySelectorAll('#attention-list .arow').length === 6, null, { timeout: wait });
      assert.equal(await page.isHidden('#attention-empty'), true);
      assert.equal(await page.innerText('#attention-count'), '6 items');
      const rows = await page.$$eval('#attention-list .arow', (r) => r.map((x) => ({
        source: x.dataset.source, word: x.querySelector('.status-word').textContent, sym: !!x.querySelector('.status-sym'),
        text: x.querySelector('.arow-text').textContent, href: x.querySelector('a').getAttribute('href'),
      })));
      assert.deepEqual(rows.map((r) => r.word), ['Critical', 'Warning', 'Warning', 'Warning', 'Warning', 'Warning'], 'critical first, then warnings');
      assert.equal(rows.every((r) => r.sym), true, 'a symbol with every word (rule 37)');
      assert.deepEqual(rows.slice(0, 3).map((r) => r.source), ['alert', 'alert', 'alert'], 'the alert rows come before the setup gaps');
      assert.deepEqual(rows.slice(3).map((r) => r.source), ['setup', 'setup', 'setup']);
      assert.deepEqual(rows.map((r) => r.href), ['/admin/audit/calls/', '/admin/notifications/#alerts-form', '/admin/audit/sealed/', '/admin/providers/#add-provider', '/admin/backup/', '/admin/network/#n-host']);
      assert.equal(rows[0].text, 'An audit record was edited.');
      // The alert rows are the bell's: their count is the badge (info is in neither).
      await page.waitForFunction(() => document.getElementById('bell-badge').textContent === '3');
      assert.equal(await page.locator('#attention-list .arow[data-source="alert"]').count(), Number(await page.innerText('#bell-badge')), 'the alert rows match the bell badge');
      assert.equal(await page.locator('.btn-primary:visible, button.primary:visible').count(), 1, 'still one filled primary');
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, 'no horizontal scroll');
      for (const box of await page.$$eval('#attention-list .arow a', (a) => a.map((x) => { const r = x.getBoundingClientRect(); return [r.width, r.height]; }))) {
        assert.ok(box[0] >= (isPhone ? 44 : 24) - 0.5 && box[1] >= (isPhone ? 44 : 24) - 0.5, 'row link target ' + box);
      }
      // Each link opens the page that fixes it.
      await page.click('#attention-list .arow[data-source="setup"] a >> nth=0');
      await page.waitForURL('**/admin/providers/**');

      // The rest of the rule table, one read at a time: a gap goes when it is fixed, and a failed read invents none.
      await page.goto(base + 'home/');
      await page.waitForSelector('#app:not([hidden])');
      setup = { ...healthy, catalog: { models: [], default: null }, users: [], clients: [], 'backup/schedule': { ...healthy['backup/schedule'], last_success_at: null, drill_due: true } };
      fake = clean;
      await page.waitForFunction(() => document.querySelectorAll('#attention-list .arow').length === 4, null, { timeout: wait });
      assert.deepEqual(await page.$$eval('#attention-list .arow a', (a) => a.map((x) => x.getAttribute('href'))),
        ['/admin/providers/#catalog-section', '/admin/people/#add-user', '/admin/backup/', '/admin/backup/'], 'no approved model, no user (so no client rule), no backup yet, drill due');
      assert.equal(await page.locator('#attention-list .arow[data-source="alert"]').count(), 0);
      setup = healthy;
      await page.waitForFunction(() => document.querySelectorAll('#attention-list .arow').length === 0, null, { timeout: wait });
      assert.equal(await page.isVisible('#attention-empty .empty-state'), true, 'the empty state returns');

      // axe with rows showing (a copy of the page in a context that lets axe in).
      fake = { ...clean, alerts: 'not_set_up' };
      setup = { ...healthy, providers: [] };
      await page.waitForFunction(() => document.querySelectorAll('#attention-list .arow').length === 2, null, { timeout: wait });
      const axeCtx = await browser.newContext({ viewport, bypassCSP: true, storageState: await ctx.storageState() });
      try {
        const ap = await axeCtx.newPage();
        await ap.route('**/api/admin/v1/status', async (route) => {
          const real = await (await route.fetch()).json();
          await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ ...real, ...fake }) });
        });
        await ap.route(/\/api\/admin\/v1\/(providers|catalog|users|clients|backup\/schedule|network)$/, async (route) => {
          const key = new URL(route.request().url()).pathname.replace('/api/admin/v1/', '');
          await route.fulfill({ contentType: 'application/json', body: JSON.stringify(setup[key]) });
        });
        await ap.goto(base + 'home/');
        await ap.waitForFunction(() => document.querySelectorAll('#attention-list .arow').length === 2, null, { timeout: wait });
        await ap.addScriptTag({ content: fs.readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8') });
        const found = await ap.evaluate(async () => (await window.axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } })).violations.map((v) => v.id + ': ' + v.nodes.map((n) => n.target.join(' ')).join(', ')));
        assert.deepEqual(found, [], 'axe on Home with attention rows');
      } finally { await axeCtx.close(); }
    } finally {
      await page.unroute('**/api/admin/v1/status');
      await page.unroute(/\/api\/admin\/v1\/(providers|catalog|users|clients|backup\/schedule|network)$/);
    }
  });
  await step('home: numbers with units right-aligned, the usage chart with a one-line summary and a hidden table of the same values, summaries that link, and greyed items that Tab reaches', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    const wait = K.LIVE_EVERY_SECS * 2 * 1000 + 3000;
    const now = Math.floor(Date.now() / 1000);
    const days = [['2026-11-10', 98], ['2026-11-11', 160], ['2026-11-12', 87]];
    let stats = {
      requests: 345, error_rate: 0.021, cost: 7.6, cost_unknown_requests: 0, tokens: { in: 1000000, out: 280000, cached: 0 },
      latency_ms: { p50: 900, p95: 6400 }, ttft_ms: { p50: 100, p95: 300 }, by_provider: [],
      by_day: days.map(([day, requests]) => ({ day, requests, tokens: { in: 0, out: 0, cached: 0 }, cost: null })),
    };
    let sinceSeen = null;
    const setup = {
      providers: [{ id: 'p1', name: 'Main' }], catalog: { models: [{ provider_id: 'p1', model: 'm' }], default: 'p1/m' }, users: [{ id: 'u1' }, { id: 'u2' }],
      clients: [{ id: 'c1' }, { id: 'c2' }, { id: 'c3' }],
      'backup/schedule': { enabled: true, destination: { path: '/b', writable: true, problem: null, same_disk: false, files: [] }, last_success_at: now - 7200, next_run_at: now + 7200, drill_due: false, missed: false },
      network: { tls_enabled: false, hostname: '' },
    };
    const services = { services: [{ name: 'server', state: 'running' }, { name: 'auditor', state: 'exited' }] };
    const setupRe = /\/api\/admin\/v1\/(providers|catalog|users|clients|backup\/schedule|network)$/;
    const install = async (target) => {
      await target.route(setupRe, async (route) => {
        if (route.request().method() !== 'GET') { await route.continue(); return; }
        await route.fulfill({ contentType: 'application/json', body: JSON.stringify(setup[new URL(route.request().url()).pathname.replace('/api/admin/v1/', '')]) });
      });
      await target.route(/\/api\/admin\/v1\/audit\/stats\?/, async (route) => {
        sinceSeen = Number(new URL(route.request().url()).searchParams.get('since'));
        await route.fulfill({ contentType: 'application/json', body: JSON.stringify(stats) });
      });
      await target.route(/\/api\/admin\/v1\/services$/, (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify(services) }));
    };
    await install(page);
    try {
      await page.goto(base + 'home/');
      await page.waitForSelector('#kpi-requests .num', { timeout: wait });
      assert.equal(Math.abs(now - sinceSeen - K.HOME_STATS_WINDOW_SECS) < 120, true, 'the stats window is HOME_STATS_WINDOW_SECS');
      assert.equal(K.HOME_STATS_WINDOW_DAYS * 86400, K.HOME_STATS_WINDOW_SECS);

      // The numbers: units, right-aligned.
      const kpis = await page.$$eval('#home-kpis .kpi', (list) => list.map((k) => ({ label: k.querySelector('.k').textContent, value: k.querySelector('.v').textContent, align: getComputedStyle(k).textAlign, num: !!k.querySelector('.v .num') })));
      assert.deepEqual(kpis.map((k) => [k.label, k.value]), [['Requests', '345'], ['Error rate', '2.1 %'], ['Tokens', '1,280,000 tokens'], ['Cost', '7.6'], ['Latency, 95th percentile', '6.4 s']]);
      assert.equal(kpis.every((k) => k.align === 'right' && k.num), true, 'every number is a right-aligned ui.num');

      // The chart: role img, its label is the visible one-line summary, and the hidden table holds the same values.
      const chart = await page.evaluate(() => {
        const c = document.getElementById('home-chart');
        const sum = document.getElementById('chart-summary').textContent;
        const t = document.getElementById('chart-table-wrap');
        const cs = getComputedStyle(document.querySelector('.chart-bar'));
        return {
          role: c.getAttribute('role'), label: c.getAttribute('aria-label'), sum, hidden: c.hidden,
          tableRows: [...document.querySelectorAll('#chart-table tbody tr')].map((r) => [...r.children].map((x) => x.textContent)),
          bars: [...document.querySelectorAll('.chart-bar')].map((b) => [b.dataset.day, Number(b.dataset.requests), b.style.height, b.querySelector('.chart-val').textContent]),
          tableBox: t.getBoundingClientRect().width, tableClass: t.className,
          anim: [cs.transitionDuration, cs.animationName],
        };
      });
      assert.equal(chart.role, 'img');
      assert.equal(chart.label, chart.sum, 'the aria-label is the summary line');
      assert.equal(chart.sum, '345 requests over 3 days, from 87 to 160 a day, busiest on Nov 11.');
      assert.deepEqual(chart.tableRows, [['Nov 10', '98'], ['Nov 11', '160'], ['Nov 12', '87']]);
      assert.deepEqual(chart.bars.map((b) => [b[1], b[3]]), [[98, '98'], [160, '160'], [87, '87']], 'the bars hold the table values');
      assert.deepEqual(chart.bars.map((b) => b[2]), ['61.25%', '100%', '54.375%'], 'heights are the share of the busiest day');
      assert.equal(chart.tableClass, 'visually-hidden');
      assert.equal(chart.anim[0], '0s', 'no transition on the bars');
      assert.equal(chart.anim[1], 'none', 'no animation on the bars');
      await page.emulateMedia({ reducedMotion: 'reduce' });
      assert.equal(await page.evaluate(() => { const cs = getComputedStyle(document.querySelector('.chart-bar')); return cs.transitionDuration + '/' + cs.animationName; }), '0s/none', 'nothing moves under reduced motion');
      await page.emulateMedia({ reducedMotion: null });

      // Summaries: services (health data), backup, clients; each links to its page.
      assert.deepEqual(await page.$$eval('#home-services-list .srow', (r) => r.map((x) => [x.firstChild.textContent, x.querySelector('.status-word').textContent, !!x.querySelector('.status-sym')])), [['Server', 'Running', true], ['auditor', 'Exited', true]]);
      assert.deepEqual(await page.$$eval('#backup-facts dt', (d) => d.map((x) => x.textContent)), ['Status', 'Last backup', 'Next backup']);
      assert.match(await page.innerText('#backup-facts'), /On schedule[\s\S]*2 hours ago[\s\S]*in 2 hours/);
      assert.deepEqual(await page.$$eval('#clients-facts dd', (d) => d.map((x) => x.textContent)), ['2', '3']);
      assert.deepEqual(await page.$$eval('.home-link', (a) => a.map((x) => x.getAttribute('href'))), ['/admin/services/', '/admin/backup/', '/admin/people/']);

      // Greyed items: five of them, each reachable by Tab, silent on Enter, with the note on focus.
      const naSel = '.kpi .na, #numbers .na, .na-sec .na-tag, #home-services .na';
      const notes = await page.$$eval(naSel, (n) => n.map((x) => [x.getAttribute('tabindex'), x.dataset.tip]));
      assert.equal(notes.length, 5, 'latency by provider, budget, spend by person, models used, uptime');
      assert.match(await page.innerText('.kpi .sub .na'), /^Family budget\s+not set/, 'the budget is a label and "not set", no invented figure');
      assert.deepEqual(await page.$$eval('.na-sec, #home-services .home-sec-head .na, #numbers .home-sec-head .na', (n) => n.map((x) => /\d/.test(x.textContent))), [false, false, false, false], 'no figures in the greyed items');
      assert.equal(notes.every(([t, tip]) => t === '0' && /^Not available yet: /.test(tip)), true);
      await page.evaluate(() => document.getElementById('home-error').focus());
      const reached = new Set();
      const urlBefore = page.url();
      for (let i = 0; i < 80 && reached.size < 5; i++) {
        await page.keyboard.press('Tab');
        const tip = await page.evaluate(() => (document.activeElement && document.activeElement.matches('[data-tip]:not(.side-menu *)') && /Not available yet/.test(document.activeElement.dataset.tip)) ? document.activeElement.dataset.tip : null);
        if (tip && !reached.has(tip)) {
          reached.add(tip);
          await page.waitForFunction(() => getComputedStyle(document.activeElement, '::after').opacity === '1', null, { timeout: 2000 }).catch(() => { throw new Error('the note does not show on focus: ' + tip); });
          await page.keyboard.press('Enter');
          assert.equal(page.url(), urlBefore, 'Enter does nothing');
          assert.equal(await page.locator('#toasts .toast').count(), 0);
        }
      }
      assert.equal(reached.size, 5, 'Tab reaches every greyed item');

      // Layout: no sideways scroll; targets.
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, 'no horizontal scroll');
      for (const box of await page.$$eval('.home-link, #numbers .na, #home-services .na, .na-sec .na-tag', (a) => a.map((x) => { const r = x.getBoundingClientRect(); return [r.width, r.height]; }))) {
        assert.ok(box[1] >= (isPhone ? 44 : 24) - 0.5 && box[0] >= 24 - 0.5, 'target ' + box);
      }

      // A quiet window: the summary says so, and there is no chart and no table.
      stats = { ...stats, requests: 0, error_rate: null, cost: null, tokens: { in: 0, out: 0, cached: 0 }, latency_ms: null, by_day: [] };
      await page.waitForFunction(() => document.getElementById('chart-summary').textContent === 'No requests in the last 7 days.', null, { timeout: wait });
      assert.equal(await page.isHidden('#home-chart'), true);
      assert.equal(await page.isHidden('#chart-table-wrap'), true);
      assert.equal(await page.innerText('#kpi-requests'), '0');
      assert.equal(await page.innerText('#kpi-latency'), '–', 'no latency is a dash, not 0');
      stats = { ...stats, requests: 345, by_day: days.map(([day, requests]) => ({ day, requests, tokens: { in: 0, out: 0, cached: 0 }, cost: null })) };

      // axe on a copy of the page that lets axe in.
      const axeCtx = await browser.newContext({ viewport, bypassCSP: true, storageState: await ctx.storageState() });
      try {
        const ap = await axeCtx.newPage();
        await install(ap);
        await ap.goto(base + 'home/');
        await ap.waitForSelector('#home-chart:not([hidden])', { timeout: wait });
        await ap.addScriptTag({ content: fs.readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8') });
        const found = await ap.evaluate(async () => (await window.axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } })).violations.map((v) => v.id + ': ' + v.nodes.map((n) => n.target.join(' ')).join(', ')));
        assert.deepEqual(found, [], 'axe on Home with numbers and chart');
      } finally { await axeCtx.close(); }
    } finally {
      await page.unroute(setupRe);
      await page.unroute(/\/api\/admin\/v1\/audit\/stats\?/);
      await page.unroute(/\/api\/admin\/v1\/services$/);
    }
  });
  await step('global search: combobox and listbox, groups in rule 4 order with Reference last, "/" focuses it but not inside a field, a setting shows its value, Enter opens it, Esc closes then clears', async () => {
    const ORDER = ['Pages', 'Settings', 'People', 'Clients', 'Providers', 'Models', 'Audit', 'Reference'];
    const state = await page.evaluate(async () => {
      const s = await (await fetch('/api/admin/v1/settings', { headers: { 'X-Askbar-Admin': '1' } })).json();
      return { timeout: s.generation_timeout_secs, theme: s.display.theme };
    });
    await page.goto(base + 'network/');
    await page.waitForSelector('#app:not([hidden])');
    const input = page.locator('#gs-input');
    // Roles and name: one combobox named "Search the admin", closed, controlling a listbox.
    assert.equal(await input.getAttribute('role'), 'combobox');
    assert.equal(await input.getAttribute('aria-expanded'), 'false');
    assert.equal(await input.getAttribute('aria-autocomplete'), 'list');
    assert.equal(await page.getAttribute('#gs-list', 'role'), 'listbox');
    assert.equal(await input.getAttribute('aria-controls'), 'gs-list');
    // "/" from the page focuses it (on a phone it opens the menu that holds it).
    await page.locator('body').press('/');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'gs-input');
    assert.equal(await input.inputValue(), '', 'the slash itself is not typed');
    assert.equal(await page.getByRole('combobox', { name: 'Search the admin' }).count(), 1);
    const box = await input.boundingBox();
    assert.ok(box.height >= (isPhone ? 44 : 24) - 0.5, 'target ' + box.height);
    if (isPhone) assert.equal(await page.evaluate(() => document.getElementById('gs').parentElement.id), 'side-menu', 'the phone menu holds it');
    else assert.ok(await page.evaluate(() => document.getElementById('gs').parentElement.classList.contains('gs-left')), 'the status bar holds it');
    // Groups come in rule 4 order, Reference never drawn while empty; the highlight is a <mark>.
    await input.fill('e');
    await page.waitForSelector('#gs-list [role="option"]');
    const heads = await page.$$eval('#gs-list [role="group"]', (g) => g.map((x) => document.getElementById(x.getAttribute('aria-labelledby')).textContent));
    assert.deepEqual(heads, ORDER.filter((h) => heads.includes(h)), 'rule 4 order');
    for (const h of ['Pages', 'Settings', 'Audit']) assert.ok(heads.includes(h), h + ' is searched');
    assert.ok(!heads.includes('Reference'), 'Reference stays out while it is empty');
    assert.equal(await input.getAttribute('aria-expanded'), 'true');
    assert.ok(await page.locator('#gs-list .gs-title mark').count() > 0, 'matched text is marked');
    // aria-activedescendant follows the arrows and names the selected option.
    const active = () => page.evaluate(() => {
      const id = document.getElementById('gs-input').getAttribute('aria-activedescendant');
      const o = document.getElementById(id);
      return { id, selected: o.getAttribute('aria-selected'), role: o.getAttribute('role'), only: document.querySelectorAll('#gs-list [aria-selected="true"]').length };
    });
    const first = await active();
    assert.deepEqual([first.selected, first.role, first.only], ['true', 'option', 1]);
    await input.press('ArrowDown');
    const second = await active();
    assert.notEqual(second.id, first.id);
    assert.equal(second.only, 1);
    await input.press('ArrowUp');
    assert.equal((await active()).id, first.id);
    // A setting shows its current value, read from the same GET the page uses.
    await input.fill('reply time limit');
    const row = page.locator('#gs-list [role="option"]', { hasText: 'Reply time limit' });
    await row.waitFor();
    assert.match(await row.innerText(), new RegExp((state.timeout === null ? 'default' : state.timeout + ' s') + '\\s*$'));
    await input.fill('theme');
    await page.locator('#gs-list [role="option"]', { hasText: 'Theme' }).waitFor();
    assert.match(await page.locator('#gs-list [role="option"]', { hasText: 'Theme' }).first().innerText(), new RegExp(state.theme + '\\s*$'));
    // Audit filters open Model calls already filtered: the address holds the filter (plan step 6.1).
    await input.fill('status: timed out');
    const timedOut = page.locator('#gs-list [role="option"]', { hasText: 'Status: timed out' });
    await timedOut.waitFor();
    assert.match(await timedOut.getAttribute('href'), /\/admin\/audit\/calls\/\?status=timed_out$/, 'the address holds the auditor\'s code, not the word');
    await input.fill('provider: ');
    const prov = page.locator('#gs-list [role="option"]', { hasText: 'Provider: ' }).first();
    await prov.waitFor();
    assert.match(await prov.getAttribute('href'), /\/admin\/audit\/calls\/\?provider=[^&]+$/);
    await input.fill('status: error');
    const audit = page.locator('#gs-list [role="option"]', { hasText: 'Status: error' });
    await audit.waitFor();
    assert.match(await audit.getAttribute('href'), /\/admin\/audit\/calls\/\?status=error$/);
    assert.match(await audit.innerText(), /filters the list/);
    // Esc closes the list and keeps the text; a second Esc clears it.
    await input.press('Escape');
    assert.equal(await input.getAttribute('aria-expanded'), 'false');
    assert.equal(await page.isHidden('#gs-pop'), true);
    assert.equal(await input.inputValue(), 'status: error');
    await input.press('Escape');
    assert.equal(await input.inputValue(), '');
    // axe with the list open (a copy of the page in a context that lets axe in).
    const axeCtx = await browser.newContext({ viewport, bypassCSP: true, storageState: await ctx.storageState() });
    try {
      const ap = await axeCtx.newPage();
      await ap.goto(base + 'network/');
      await ap.waitForSelector('#app:not([hidden])');
      await ap.locator('body').press('/');
      await ap.fill('#gs-input', 'e');
      await ap.waitForSelector('#gs-list [role="option"]');
      await ap.addScriptTag({ content: fs.readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8') });
      const found = await ap.evaluate(async () => (await window.axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } })).violations.map((v) => v.id + ': ' + v.nodes.map((n) => n.target.join(' ')).join(', ')));
      assert.deepEqual(found, [], 'axe with the list open');
    } finally { await axeCtx.close(); }
    // Enter opens the page at the field, which takes focus.
    await input.fill('reply time limit');
    await page.locator('#gs-list [role="option"]', { hasText: 'Reply time limit' }).waitFor();
    await input.press('Enter');
    await page.waitForURL('**/admin/general/#s-gen-timeout');
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForFunction(() => document.activeElement && document.activeElement.id === 's-gen-timeout');
    // "/" typed inside a field is a character, not the shortcut.
    await page.keyboard.type('/');
    assert.equal(await page.evaluate(() => document.activeElement.id), 's-gen-timeout');
    await page.evaluate(() => { document.getElementById('s-gen-timeout').value = ''; });
  });
  await step('audit trace: the waterfall layout is right for fixed numbers, shows the tree, and copes with a running span', async () => {
    await openPage('audit/calls');
    const spans = [
      { id: 'a-turn', parent_id: null, kind: 'turn', name: 'chat', status: 'done', started_at: 1000, ended_at: 1002, attrs: { started_ms: 1000000, ended_ms: 1002000 } },
      { id: 'b-attempt', parent_id: 'a-turn', kind: 'attempt', name: 'prov', status: 'error', started_at: 1000, ended_at: 1000, provider_id: 'p1', model: 'm', error_code: 'http_429', attrs: { started_ms: 1000200, ended_ms: 1000700, retry_index: 0 } },
      { id: 'c-attempt', parent_id: 'a-turn', kind: 'attempt', name: 'prov', status: 'done', started_at: 1001, ended_at: 1002, provider_id: 'p1', model: 'm', tokens_in: 100, tokens_out: 50, ttft_ms: 300, attrs: { started_ms: 1001000, ended_ms: 1001900, retry_index: 1 } },
      { id: '0-gate', parent_id: null, kind: 'gate', name: 'kill_switch', status: 'done', started_at: 1000, ended_at: 1000, attrs: { started_ms: 999990, ended_ms: 1000000 } },
      { id: 'd-running', parent_id: 'a-turn', kind: 'tool', name: 'still going', status: 'running', started_at: 1001, ended_at: null, attrs: { started_ms: 1001500 } },
    ];
    const lay = await page.evaluate((s) => window.askbarAudit.layout(s, 1002500), spans);
    // Ordered by when they began: the gate first (it began 10 ms before the turn), then turn, attempt, attempt, tool.
    assert.deepEqual(lay.rows.map((r) => r.span.id), ['0-gate', 'a-turn', 'b-attempt', 'c-attempt', 'd-running']);
    assert.deepEqual(lay.rows.map((r) => r.depth), [0, 0, 1, 1, 1], 'attempts and the tool sit under the turn');
    assert.equal(lay.start, 999990);
    assert.equal(lay.end, 1002500, 'the running span is drawn up to now, which is the latest end');
    assert.equal(lay.total, 2510);
    const by = Object.fromEntries(lay.rows.map((r) => [r.span.id, r]));
    assert.deepEqual([by['0-gate'].left, by['0-gate'].width], [0, 0.5], 'a 10 ms span of 2510 would be under half a percent: it keeps a visible minimum');
    assert.deepEqual([by['a-turn'].left, by['a-turn'].width], [0.4, 79.68]);
    assert.deepEqual([by['b-attempt'].left, by['b-attempt'].width], [8.37, 19.92]);
    assert.deepEqual([by['c-attempt'].left, by['c-attempt'].width], [40.24, 35.86]);
    assert.equal(by['d-running'].running, true);
    assert.deepEqual([by['d-running'].left, by['d-running'].width], [60.16, 39.84], 'ends at the right edge');
    assert.equal(by['d-running'].durationMs, 1000);
    // Nothing at all lays out to nothing, without dividing by zero.
    assert.deepEqual(await page.evaluate(() => window.askbarAudit.layout([], 5).rows), []);

    // Drawn: the tree, the bars, and details on focus. (Without the running span, whose end is the
    // real clock, so the widths below are exact.)
    const finished = spans.filter((x) => x.status !== 'running');
    await page.route('**/api/admin/v1/audit/traces/TRACE1', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ trace_id: 'TRACE1', cost: 0.0007, spans: finished }) }));
    await openPage('providers');
    await openPage('audit/calls');
    await page.evaluate(() => window.askbarAudit.openTrace('TRACE1'));
    await page.waitForSelector('#trace-waterfall .span-bar');
    assert.equal(await page.locator('#trace-waterfall .span-bar').count(), 4);
    assert.equal(await page.locator('#trace-waterfall .span-bar.status-error').count(), 1);
    assert.equal(await page.innerText('#trace-id'), 'TRACE1');
    assert.match(await page.innerText('#trace-state'), /done/);
    assert.match(await page.innerText('#trace-meta'), /0\.0007/);
    // total = 1002000 - 999990 = 2010 ms; c-attempt starts 1010 ms in and runs 900 ms.
    assert.equal(await page.$eval('[data-span="c-attempt"]', (b) => b.style.width), '44.78%');
    assert.equal(await page.$eval('[data-span="c-attempt"]', (b) => b.style.left), '50.25%');
    assert.equal(await page.$eval('[data-span="b-attempt"]', (b) => b.closest('.span-row').querySelector('.label').style.paddingLeft), '14px');
    await page.focus('[data-span="b-attempt"]');
    const details = await page.innerText('#trace-span-details');
    assert.match(details, /attempt · prov · error/);
    assert.match(details, /starts at \+210 ms, takes 500 ms/);
    assert.match(details, /error http_429/);
    assert.match(details, /"retry_index":0/);
    // A running span is drawn in its own style, up to now.
    await page.route('**/api/admin/v1/audit/traces/TRACE2', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ trace_id: 'TRACE2', cost: null, spans }) }));
    await page.evaluate(() => window.askbarAudit.openTrace('TRACE2'));
    await page.waitForSelector('#trace-waterfall .span-bar.running');
    assert.equal(await page.locator('#trace-waterfall .span-bar.running').count(), 1);
    assert.equal(await page.locator('#trace-waterfall .span-bar').count(), 5);
    await page.unroute('**/api/admin/v1/audit/traces/TRACE2');
    await page.unroute('**/api/admin/v1/audit/traces/TRACE1');
  });
  await step('audit routing: a three-try request shows its tries, why each failed, and the providers overall', async () => {
    const attempt = (id, started, provider, retry, status, code) => ({ id, parent_id: 'turn', kind: 'attempt', name: 'x', status, started_at: 1000, ended_at: 1001, provider_id: provider, error_code: code, attrs: { started_ms: started, ended_ms: started + 300, latency_ms: 300, retry_index: retry } });
    const spans = [
      { id: 'turn', parent_id: null, kind: 'turn', name: 'chat', status: 'done', started_at: 1000, ended_at: 1002, attrs: { started_ms: 1000000, ended_ms: 1002000 } },
      attempt('a3', 1001500, 'p1', 2, 'done', null),
      attempt('a1', 1000100, 'p1', 0, 'error', 'http_429'),
      attempt('a2', 1000800, 'p1', 1, 'error', 'http_429'),
    ];
    const stats = { requests: 1, by_provider: [
      { provider_id: 'p1', requests: 1, attempts: 3, retries: 2, failed_attempts: 2, tokens: { in: 1, out: 1, cached: 0 }, cost: 0 },
      { provider_id: 'p2', requests: 0, attempts: 1, retries: 0, failed_attempts: 1, tokens: { in: 0, out: 0, cached: 0 }, cost: null },
    ], by_day: [], tokens: { in: 1, out: 1, cached: 0 }, latency_ms: {}, ttft_ms: {} };
    await page.route('**/api/admin/v1/audit/traces/ROUTE1', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ trace_id: 'ROUTE1', cost: 0, spans }) }));
    await page.route('**/api/admin/v1/audit/stats*', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify(stats) }));
    await page.route('**/api/admin/v1/audit/provider-health', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ providers: [{ provider_id: 'p1', status: 'auth_failed', since: 1793631845, error_code: 'http_401' }] }) }));
    await openPage('providers');
    await page.goto(base + 'audit/routing/?id=ROUTE1');
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#audit-attempt-rows tr');
    const rows = await page.$$eval('#audit-attempt-rows tr', (r) => r.map((tr) => [...tr.children].map((td) => td.textContent)));
    assert.deepEqual(rows, [
      ['p1', '1', 'error', 'worth retrying', 'http_429', '300 ms'],
      ['p1', '2', 'error', 'worth retrying', 'http_429', '300 ms'],
      ['p1', '3', 'done', '–', '–', '300 ms'],
    ]);
    // The rows of an earlier step may still be there; wait for the ones this step's canned stats draw.
    await page.waitForSelector('#audit-routing-rows tr[data-provider="p2"]');
    const overall = await page.$$eval('#audit-routing-rows tr', (r) => r.map((tr) => [...tr.children].map((td) => td.textContent)));
    assert.equal(overall.length, 2);
    assert.deepEqual(overall[0].slice(0, 4), ['p1', '3', '2', '2']);
    assert.match(overall[0][4], /^refused since .* \(http_401\)$/);
    assert.deepEqual(overall[1], ['p2', '1', '0', '1', 'ok']);
    // The classes follow the router's own rules.
    const classes = await page.evaluate(() => ['http_401', 'http_403', 'config', 'http_429', 'http_503', 'connect', 'stream', 'http_400', 'http_404', 'timeout', 'kill_switch', null].map((c) => window.askbarAudit.failureClass(c)));
    assert.deepEqual(classes, ['bad credentials', 'bad credentials', 'bad credentials', 'worth retrying', 'worth retrying', 'worth retrying', 'worth retrying', 'refused outright', 'refused outright', 'ran out of time', 'switched off', '']);
    await page.unroute('**/api/admin/v1/audit/traces/ROUTE1');
    await page.unroute('**/api/admin/v1/audit/stats*');
    await page.unroute('**/api/admin/v1/audit/provider-health');
  });
  await step('audit calls (canned): sealed content stays sealed in the trace panel until the passphrase opens a session, View shows text as text, and a session that ends wipes it', async () => {
    const spans = [
      { id: 'sp-turn', parent_id: null, kind: 'turn', name: 'chat', status: 'done', started_at: 1000, ended_at: 1001, sealed: 1, attrs: {} },
      { id: 'sp-attempt', parent_id: 'sp-turn', kind: 'attempt', name: 'prov', status: 'done', started_at: 1000, ended_at: 1001, sealed: 2, attrs: {} },
      { id: 'sp-gate', parent_id: null, kind: 'gate', name: 'kill_switch', status: 'done', started_at: 1000, ended_at: 1000, sealed: 0, attrs: {} },
    ];
    let open = false;
    const viewed = [];
    const sent = [];
    const sessionBody = () => ({ open, idle_left_secs: open ? 900 : null, total_left_secs: open ? 3600 : null, idle_limit_secs: 900, total_limit_secs: 3600, keys: [{ key_id: 'k1', created_at: 1000, retired_at: null, open }] });
    await page.route('**/api/admin/v1/audit/traces/SEAL1', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ trace_id: 'SEAL1', cost: null, spans }) }));
    await page.route('**/api/admin/v1/audit/session', (route) => {
      const m = route.request().method();
      if (m === 'POST') {
        const body = JSON.parse(route.request().postData());
        sent.push(body.passphrase);
        if (body.passphrase !== 'the right passphrase here') return route.fulfill({ status: 403, contentType: 'application/json', body: JSON.stringify({ error: 'forbidden', detail: 'That passphrase does not open any audit key.' }) });
        open = true;
        return route.fulfill({ status: 201, contentType: 'application/json', body: JSON.stringify(sessionBody()) });
      }
      if (m === 'DELETE') { open = false; return route.fulfill({ status: 204 }); }
      route.fulfill({ contentType: 'application/json', body: JSON.stringify(sessionBody()) });
    });
    await page.route('**/api/admin/v1/audit/payloads/*', (route) => {
      viewed.push(route.request().url());
      if (!open) return route.fulfill({ status: 403, contentType: 'application/json', body: JSON.stringify({ error: 'forbidden', detail: 'Sealed content is sealed.' }) });
      route.fulfill({ contentType: 'application/json', body: JSON.stringify({ span_id: 'sp-attempt', payloads: [{ role: 'request', key_id: 'k1', text: 'the <b>question</b> that was asked' }, { role: 'error', key_id: 'k0', text: null, key_created_at: 500, key_retired_at: 900 }] }) });
    });
    await openPage('audit/calls');
    await page.evaluate(() => window.askbarAudit.openTrace('SEAL1'));
    await page.waitForSelector('#trace-sealed .sealed-row');
    // Sealed, with a View button only for spans that have something; nothing fetched yet.
    assert.equal(await page.locator('#trace-sealed .sealed-row').count(), 2);
    assert.match(await page.innerText('#trace-sealed'), /Sealed \(1 item\)/);
    assert.match(await page.innerText('#trace-sealed'), /Sealed \(2 items\)/);
    assert.equal(viewed.length, 0, 'nothing is fetched before a session is open');
    // View while sealed: the way to the passphrase (the status bar's Unseal box), and still nothing fetched.
    await page.click('#trace-sealed [data-span="sp-attempt"] .view-btn');
    await page.waitForSelector('#unseal-box[open]');
    assert.match(await page.innerText('#trace-sealed-error'), /Enter the audit passphrase/);
    assert.equal(viewed.length, 0);
    // Wrong: the server's sentence, nothing opened, the field emptied.
    await page.fill('#unseal-pass', 'a wrong passphrase of length');
    await page.click('#unseal-go');
    await page.waitForFunction(() => /does not open any audit key/.test(document.getElementById('unseal-pass-error').textContent));
    assert.equal(await page.inputValue('#unseal-pass'), '', 'the field is emptied');
    // Right: the session opens, the box closes, and the item asked for opens with it; no prompt for each item.
    await page.fill('#unseal-pass', 'the right passphrase here');
    await page.click('#unseal-go');
    await page.waitForSelector('#unseal-box', { state: 'hidden' });
    await page.waitForFunction(() => /that was asked/.test(document.getElementById('trace-viewed').textContent));
    const plain = await page.innerText('#trace-viewed');
    assert.match(plain, /request \(key k1\)/);
    assert.match(plain, /the <b>question<\/b> that was asked/, 'markup is shown as typed');
    assert.equal(await page.locator('#trace-viewed b').count(), 0);
    assert.match(plain, /not opened: this session has no key for it; key k0 was made .* and retired/);
    assert.ok(viewed.every((u) => /sp-attempt$/.test(u)));
    assert.deepEqual(sent, ['a wrong passphrase of length', 'the right passphrase here']);
    // The session ends on its own (a timeout, sign-out elsewhere): the panel is wiped and asks again.
    open = false;
    await page.evaluate(() => window.askbarAudit.sessionFromStatus({ open: false }));
    await page.waitForFunction(() => document.getElementById('trace-viewed').children.length === 0);
    assert.equal((await page.content()).includes('that was asked'), false, 'wiped when the session ended');
    // A view that finds it sealed says so, in the panel.
    await page.evaluate(() => { window.askbarAudit.session.open = true; });
    await page.click('#trace-sealed [data-span="sp-attempt"] .view-btn');
    await page.waitForFunction(() => /sealed/i.test(document.getElementById('trace-sealed-error').textContent));
    assert.equal(await page.locator('#tab-audit-calls img').count(), 0);
    assert.equal(await page.evaluate(() => window.__pwned), undefined);
    await page.unroute('**/api/admin/v1/audit/traces/SEAL1');
    await page.unroute('**/api/admin/v1/audit/session');
    await page.unroute('**/api/admin/v1/audit/payloads/*');
  });
  await step('audit access history: rows are listed newest first as text, and a new open adds one', async () => {
    let rows = [
      { id: 2, span_id: 'span-<b>x</b>', key_id: '0011223344556677', opened_at: 1793631845, ok: false },
      { id: 1, span_id: 'span-1', key_id: '0011223344556677', opened_at: 1793631800, ok: true },
    ];
    await page.route('**/api/admin/v1/audit/access*', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ access: rows }) }));
    await openPage('audit/sealed');
    await page.waitForSelector('#audit-access-rows tr');
    const cells = await page.$$eval('#audit-access-rows tr', (r) => r.map((tr) => [...tr.children].map((td) => td.textContent).slice(1, 4)));
    assert.deepEqual(cells, [['span-<b>x</b>', '0011223344556677', 'wrong passphrase'], ['span-1', '0011223344556677', 'opened']]);
    assert.equal(await page.locator('#audit-access b').count(), 0, 'shown as text');
    assert.equal(await page.isHidden('#audit-access-empty'), true);
    rows = [];
    await page.reload();
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#audit-access-empty:not([hidden])');
    assert.equal(await page.isHidden('#audit-access'), true);
    await page.unroute('**/api/admin/v1/audit/access*');
  });
  await step('audit settings: the key id and retention show, bad values give their messages, and rotating works from the page', async () => {
    await openSettings();
    await page.waitForFunction(() => /^[0-9a-f]{16}$/.test(document.getElementById('s-key-id').textContent));
    const oldKey = await page.innerText('#s-key-id');
    assert.equal(await page.inputValue('#s-retention'), '30');
    assert.equal(await page.getAttribute('#s-retention', 'min'), '7');

    // Retention: the server's message for a value that is too small.
    await page.evaluate(() => document.getElementById('settings-form').noValidate = true);
    await page.fill('#s-retention', '5');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#settings-error:not([hidden])');
    assert.match(await page.innerText('#settings-error'), /at least 7 days/);
    await page.fill('#s-retention', '45');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    await page.fill('#s-retention', '999'); // unsaved, so the reload shows what was stored
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-retention').value === '45');

    // Rotation (on the Security page): each refusal in turn, then success.
    await openPage('security');
    await page.waitForFunction((k) => document.getElementById('s-key-id').textContent === k, oldKey);
    const NEW_PASS = 'a different long passphrase 77';
    const rotate = async (cur, next, next2) => {
      await page.fill('#r-current', cur);
      await page.fill('#r-new', next);
      await page.fill('#r-new2', next2);
      await page.click('#rotate-form button[type=submit]');
    };
    await rotate(AUDIT_PASS, NEW_PASS, NEW_PASS + 'x');
    assert.match(await page.innerText('#rotate-form .error-summary'), /do not match/);
    assert.match(await page.innerText('#r-new2-error'), /do not match/);
    await rotate(AUDIT_PASS, 'too short', 'too short');
    assert.match(await page.innerText('#rotate-form .error-summary'), /at least 20/);
    await rotate('not the current passphrase!!', NEW_PASS, NEW_PASS);
    await page.waitForFunction(() => /current passphrase is wrong/.test(document.getElementById('rotate-error').textContent));
    assert.equal(await page.inputValue('#r-current'), '', 'the current passphrase is cleared after a try');
    assert.equal(await page.innerText('#s-key-id'), oldKey, 'nothing changed');
    await rotate(AUDIT_PASS, NEW_PASS, NEW_PASS);
    await page.waitForSelector('#toasts .toast');
    assert.match(await page.locator('#toasts .toast').last().innerText(), /audit key was rotated/);
    const newKey = await page.innerText('#s-key-id');
    assert.match(newKey, /^[0-9a-f]{16}$/);
    assert.notEqual(newKey, oldKey);
    for (const id of ['r-current', 'r-new', 'r-new2']) assert.equal(await page.inputValue('#' + id), '', id);
    assert.equal((await page.content()).includes(NEW_PASS), false);
    // Put the retention back so later steps see an ordinary server.
    await openSettings();
    await page.fill('#s-retention', '30');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
  });
  await step('security: changing the password needs the current one', async () => {
    await openPage('security');
    await page.fill('#pw-current', 'wrong');
    await page.fill('#pw-new', 'brand new password 1');
    await page.fill('#pw-new2', 'brand new password 1');
    await page.click('#password-form button[type=submit]');
    await page.waitForSelector('#pw-error:not([hidden])');
    assert.match(await page.innerText('#pw-error'), /current password is wrong/);
    assert.equal(await page.isVisible('#app'), true, 'a wrong current password does not sign you out');
    await page.fill('#pw-current', PASSWORD);
    await page.click('#password-form button[type=submit]');
    await page.waitForSelector('#toasts .toast');
    assert.match(await page.locator('#toasts .toast').last().innerText(), /Password changed/);
  });
  await step('admin API key: shown once, then only its name; a script can use it until it is revoked', async () => {
    await openPage('security');
    await page.waitForSelector('#key-form:not([hidden])');
    await page.fill('#key-name', 'build server');
    await page.click('#key-create');
    await page.waitForSelector('#key-dialog[open]');
    const key = (await page.innerText('#key-value')).trim();
    assert.match(key, /^askbar-admin-[A-Za-z0-9_-]{43}$/);
    assert.match(await page.innerText('#key-dialog'), /only time the key is shown/);
    const call = (k, method = 'GET', path = '/api/admin/v1/providers') => fetch(`http://127.0.0.1:${adminPort}${path}`, { method, headers: { authorization: 'Bearer ' + k } });
    assert.equal((await call(key)).status, 200, 'a script can use the key');
    assert.equal((await call(key, 'GET', '/api/admin/v1/keys')).status, 401, 'but not to manage keys');
    await page.click('#key-close');
    assert.equal(await page.locator('#key-dialog[open]').count(), 0);
    assert.ok(!(await page.content()).includes(key), 'the key is not on the page any more');
    assert.equal(await page.isVisible('#key-form'), false, 'one key at a time: no form while one exists');
    assert.match(await page.innerText('#key-row'), /build server/);
    // After a reload it is still only a name.
    await page.reload();
    await page.waitForSelector('#app:not([hidden])');
    await openPage('security');
    await page.waitForSelector('#key-row:not([hidden])');
    assert.ok(!(await page.content()).includes(key));
    // Revoking asks twice, then it stops working and a new one can be made.
    await page.click('#key-revoke');
    assert.match(await page.innerText('#key-revoke'), /stops working/);
    await page.click('#key-revoke');
    await page.waitForSelector('#key-form:not([hidden])');
    assert.equal((await call(key)).status, 401, 'revoked');
  });
  await step('security: the password pair shows a live count from constants and a mismatch blocks the request', async () => {
    await openPage('security');
    const min = await page.evaluate(async () => (await window.ui.ready).MIN_PASSWORD_CHARS);
    assert.equal(await page.getAttribute('#pw-new', 'minlength'), String(min));
    assert.equal((await page.innerText('#pw-new-hint')).trim(), 'At least ' + min + ' characters');
    await page.fill('#pw-new', 'abc');
    assert.equal((await page.innerText('#pw-new-hint')).trim(), '3 of at least ' + min + ' characters');
    await page.fill('#pw-new', 'x'.repeat(min));
    assert.equal((await page.innerText('#pw-new-hint')).trim(), min + ' characters');
    assert.equal(await page.evaluate(() => document.getElementById('pw-new-hint').classList.contains('length-ok')), true);
    const stacked = await page.evaluate(() => {
      const a = document.getElementById('pw-new').getBoundingClientRect();
      const b = document.getElementById('pw-new2').getBoundingClientRect();
      return b.top > a.bottom && Math.abs(a.left - b.left) < 1 && Math.abs(a.width - b.width) < 1;
    });
    assert.equal(stacked, true, 'new and repeat are stacked at the same width');
    let sent = 0;
    const watch = (r) => { if (r.url().endsWith('/api/admin/v1/password')) sent++; };
    page.on('request', watch);
    await page.fill('#pw-current', 'anything');
    await page.fill('#pw-new2', 'y'.repeat(min));
    await page.click('#password-form button[type=submit]');
    await page.waitForSelector('#pw-new2-error:not([hidden])');
    assert.match(await page.innerText('#pw-new2-error'), /do not match/);
    assert.equal(sent, 0, 'a mismatch sends nothing');
    page.off('request', watch);
    for (const id of ['#pw-current', '#pw-new', '#pw-new2']) await page.fill(id, '');
  });
  await step('security: the passkey and recovery codes are greyed with aria-disabled, reachable by Tab, inert on Enter and show no numbers', async () => {
    await openPage('security');
    const na = page.locator('#main .na');
    assert.equal(await na.count(), 2);
    assert.equal(await page.locator('#main .na[disabled], #main .na [disabled]').count(), 0, 'never the disabled attribute');
    for (const [i, what] of [[0, /passkey/], [1, /recovery codes/]]) {
      const one = na.nth(i);
      assert.equal(await one.getAttribute('aria-disabled'), 'true');
      assert.equal(await one.getAttribute('tabindex'), '0');
      assert.match(await one.getAttribute('data-tip'), /^Not available yet: .*/);
      assert.match(await one.getAttribute('data-tip'), what);
      await one.focus();
      assert.equal(await one.evaluate((e) => e === document.activeElement), true);
      const url = page.url();
      await page.keyboard.press('Enter');
      assert.equal(page.url(), url);
      assert.equal(await page.locator('dialog[open]').count(), 0, 'Enter does nothing');
    }
    for (const id of ['passkey-title', 'recovery-title']) {
      const text = await page.evaluate((i) => document.getElementById(i).closest('section').innerText, id);
      assert.ok(!/\d/.test(text), 'no invented numbers: ' + text);
    }
  });
  await step('security: a new API key is shown once in a box and is gone from the page once the box closes (also by Escape)', async () => {
    await openPage('security');
    await page.waitForSelector('#key-form:not([hidden])');
    const max = await page.evaluate(async () => (await window.ui.ready).API_KEY_MAX_NAME_CHARS);
    assert.equal(await page.getAttribute('#key-name', 'maxlength'), String(max));
    await page.fill('#key-name', 'second server');
    await page.click('#key-create');
    await page.waitForSelector('#key-dialog[open]');
    const key = (await page.innerText('#key-value')).trim();
    assert.match(key, /^askbar-admin-/);
    assert.equal(await page.locator('#key-dialog [data-copy="key-value"]').count(), 1, 'a copy button sits beside the key');
    assert.equal(await page.locator('#main code#key-value').count(), 1);
    await page.keyboard.press('Escape');
    assert.equal(await page.locator('#key-dialog[open]').count(), 0);
    assert.equal(await page.evaluate(() => document.getElementById('key-value').textContent), '');
    assert.ok(!(await page.content()).includes(key), 'the key is not in the page after the box closes');
    await page.waitForSelector('#key-row:not([hidden])');
    assert.match(await page.innerText('#key-table'), /second server/);
    assert.equal(await page.locator('#key-table tbody tr').count(), 1, 'one key');
    await page.click('#key-revoke');
    await page.click('#key-revoke');
    await page.waitForSelector('#key-form:not([hidden])');
  });
  await step('security: the sealed-content times are read from the settings and the audit key id shows', async () => {
    await openPage('security');
    const s = await page.evaluate(async () => (await fetch('/api/admin/v1/settings')).json());
    await page.waitForFunction(() => document.getElementById('sec-seal-idle').textContent !== '');
    const idle = await page.innerText('#sec-seal-idle');
    const total = await page.innerText('#sec-seal-total');
    assert.equal(parseInt(idle, 10), s.unseal_idle_minutes);
    assert.equal(parseInt(total, 10), s.unseal_total_minutes);
    assert.match(await page.innerText('#s-key-id'), /^[0-9a-f]{16}$/);
    assert.equal(await page.locator('#rotate-form #r-new + .hint[data-length-hint]').count(), 1);
    assert.equal(await page.getAttribute('#r-new', 'minlength'), String(await page.evaluate(async () => (await window.ui.ready).MIN_PASSPHRASE_CHARS)));
  });
  await step('security: nothing scrolls sideways and every target is 44 px tall on a phone', async () => {
    await openPage('security');
    await page.waitForSelector('#pw-title');
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, 'no sideways scroll');
    if (isPhone) {
      const small = await page.evaluate(() => [...document.querySelectorAll('#main button, #main input, #main a, #main .na')]
        .filter((e) => e.offsetParent && !e.closest('[hidden]'))
        .map((e) => [e.id || e.textContent.trim().slice(0, 20), e.getBoundingClientRect().height])
        .filter(([, h]) => h > 0 && h < 44));
      assert.deepEqual(small, []);
    }
  });
  // Shared accessibility behaviour (plan step 1.10; rules 26, 41-46).
  await step('accessibility: Skip to content is the first Tab stop, and Enter moves focus to the page', async () => {
    await page.goto(base + 'security/');
    await page.waitForSelector('#app:not([hidden])');
    await page.keyboard.press('Tab');
    const first = await page.evaluate(() => {
      const a = document.activeElement;
      const r = a.getBoundingClientRect();
      return { cls: a.className, text: a.textContent, onScreen: r.top >= 0 && r.bottom <= innerHeight && r.width > 0 };
    });
    assert.deepEqual(first, { cls: 'skip-link', text: 'Skip to content', onScreen: true });
    await page.keyboard.press('Enter');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'main');
  });
  await step('accessibility: a failed save shows a summary at the top and moves focus to the first field with a problem', async () => {
    await page.fill('#pw-current', 'anything');
    await page.fill('#pw-new', 'one new password');
    await page.fill('#pw-new2', 'another password');
    await page.click('#password-form button[type=submit]');
    await page.waitForSelector('#password-form .error-summary[role=alert]');
    assert.match(await page.innerText('#password-form .error-summary'), /do not match/);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'pw-new2', 'focus is on the field to fix');
    assert.equal(await page.getAttribute('#pw-new2', 'aria-invalid'), 'true');
    assert.match(await page.innerText('#pw-new2-error'), /do not match/);
    assert.ok((await page.getAttribute('#pw-new2', 'aria-describedby')).includes('pw-new2-error'), 'the error describes the field');
    await page.fill('#pw-new2', 'one new password');
    await page.fill('#pw-current', 'wrong');
    await page.click('#password-form button[type=submit]');
    await page.waitForSelector('#pw-error:not([hidden])');
    assert.equal(await page.locator('#password-form .error-summary').count(), 0, 'the summary goes on the next try');
    assert.equal(await page.getAttribute('#pw-new2', 'aria-invalid'), null);
    for (const id of ['#pw-current', '#pw-new', '#pw-new2']) await page.fill(id, '');
  });
  await step('accessibility: one announcer reads updates without moving focus, assertive only when urgent', async () => {
    const r = await page.evaluate(async () => {
      const b = document.querySelector('#password-form button[type=submit]');
      b.focus();
      window.ui.announce('Polite news');
      window.ui.announce('Urgent news', { urgent: true });
      await new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok)));
      return {
        focus: document.activeElement === b,
        polite: [document.getElementById('announcer').textContent, document.getElementById('announcer').getAttribute('aria-live')],
        urgent: [document.getElementById('announcer-urgent').textContent, document.getElementById('announcer-urgent').getAttribute('aria-live')],
        regions: document.querySelectorAll('[aria-live]').length,
      };
    });
    assert.deepEqual(r, { focus: true, polite: ['Polite news', 'polite'], urgent: ['Urgent news', 'assertive'], regions: 2 });
  });
  await step('accessibility: focus scrolls clear of the status bar, and nothing moves under reduced motion', async () => {
    const pad = await page.evaluate(() => getComputedStyle(document.documentElement).scrollPaddingTop);
    const statusH = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--status-h')));
    assert.ok(parseFloat(pad) >= statusH, 'scroll-padding-top ' + pad + ' covers the ' + statusH + ' px status bar');
    await page.emulateMedia({ reducedMotion: 'reduce' });
    const still = await page.evaluate(() => {
      const { el } = window.ui;
      const probe = el('div', { id: 't-motion' }, el('span', { class: 'skeleton-row' }), el('button', { type: 'button', class: 'btn btn-icon', 'data-tip': 'Probe', 'aria-label': 'Probe' }));
      document.querySelector('#app main').append(probe);
      const out = {
        animations: document.getAnimations().length,
        skeleton: getComputedStyle(probe.querySelector('.skeleton-row')).animationName,
        tip: getComputedStyle(probe.querySelector('.btn-icon'), '::after').transitionDuration,
      };
      probe.remove();
      return out;
    });
    await page.emulateMedia({ reducedMotion: null });
    assert.deepEqual(still, { animations: 0, skeleton: 'none', tip: '0s' });
  });
  await step(isPhone ? 'accessibility: every target on every page is at least 44 px on a phone' : 'accessibility: every target on every page is at least 24 px', async () => {
    const min = isPhone ? 44 : 24;
    const slugs = await page.$$eval('#nav a[data-tab]', (as) => as.map((a) => a.dataset.tab));
    const small = [];
    for (const slug of slugs) {
      await page.goto(base + slug + '/');
      await page.waitForSelector('#app:not([hidden])');
      await page.waitForLoadState('networkidle');
      small.push(...await page.evaluate((m) => {
        // Controls a person aims at. A link inside running text is exempt (WCAG 2.5.8, inline); a checkbox is aimed at
        // through its label, so the label is measured instead.
        const sel = 'button, input:not([type=checkbox]):not([type=radio]):not([type=hidden]), select, textarea, label.check, nav a, a.button, a.btn, .skip-link, [tabindex="0"]';
        return [...document.querySelectorAll(sel)].filter((n) => {
          const r = n.getBoundingClientRect();
          if (r.width === 0 && r.height === 0) return false;
          if (n.matches('.skip-link')) return false;
          return r.height < m - 0.5 || r.width < 24 - 0.5;
        }).map((n) => location.pathname + ' ' + n.tagName.toLowerCase() + (n.id ? '#' + n.id : '') + '.' + n.className + ' ' + Math.round(n.getBoundingClientRect().width) + 'x' + Math.round(n.getBoundingClientRect().height));
      }, min));
    }
    assert.deepEqual(small, []);
    await page.goto(base);
    await page.waitForSelector('#app:not([hidden])');
  });
  // axe-core (plan step 1.11): every page, signed in and signed out, at both widths. A violation whose rule is not in
  // tests/e2e/axe-baseline.json for that page fails; the baseline only shrinks. AXE_BASELINE=write records what is found
  // now (the union over both widths) instead of failing. axe is injected in a second context that bypasses the CSP, so
  // the main page keeps its CSP and Trusted Types for every other step.
  await step('accessibility: axe finds no violation beyond the baseline on any page', async () => {
    const baselineFile = path.join(__dirname, 'axe-baseline.json');
    const write = process.env.AXE_BASELINE === 'write';
    const baseline = fs.existsSync(baselineFile) ? JSON.parse(fs.readFileSync(baselineFile, 'utf8')) : {};
    const axeSource = fs.readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8');
    const scan = async (context, url, key) => {
      const p = await context.newPage();
      try { return await scanPage(p, url, key); } finally { await p.close(); }
    };
    const scanPage = async (p, url, key) => {
      await p.goto(url);
      await p.waitForSelector(key === 'login' ? '#login:not([hidden])' : '#app:not([hidden])');
      // A page that polls (Services) is never idle for long; give the loaders a moment and scan what is there.
      await p.waitForLoadState('networkidle', { timeout: 5000 }).catch(() => {});
      await p.addScriptTag({ content: axeSource });
      const found = await p.evaluate(async () => {
        const r = await window.axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } });
        return r.violations.map((v) => ({ id: v.id, nodes: v.nodes.map((n) => n.target.join(' ')).slice(0, 5) }));
      });
      return found;
    };
    const signedIn = await browser.newContext({ viewport, bypassCSP: true, storageState: await ctx.storageState() });
    const signedOut = await browser.newContext({ viewport, bypassCSP: true });
    const slugs = await page.$$eval('#nav a[data-tab]', (as) => as.map((a) => a.dataset.tab));
    const extra = [];
    try {
      for (const key of [...slugs, 'login']) {
        const found = key === 'login' ? await scan(signedOut, base, key) : await scan(signedIn, base + key + '/', key);
        const known = new Set(baseline[key] || []);
        if (write) baseline[key] = [...new Set([...known, ...found.map((v) => v.id)])].sort();
        else for (const v of found) if (!known.has(v.id)) extra.push(key + ': ' + v.id + ' at ' + v.nodes.join(', '));
      }
    } finally {
      await signedIn.close();
      await signedOut.close();
    }
    if (write) fs.writeFileSync(baselineFile, JSON.stringify(baseline, null, 2) + '\n');
    assert.deepEqual(extra, [], 'new axe violations (fix them; the baseline only shrinks)');
  });
  await step('pages: each section has its own URL, the nav marks it, Back works and the frame is the same', async () => {
    // Plan step 1.16: a page and a URL for every admin section, from one template.
    const slugs = await page.$$eval('#nav a[data-tab]', (as) => as.map((a) => a.dataset.tab));
    assert.deepEqual(slugs, ['home', 'alerts', 'audit/calls', 'audit/admin', 'audit/sealed', 'audit/routing', 'settings', 'providers', 'people', 'services', 'network', 'security', 'backup', 'notifications', 'general', 'reference']);
    for (const slug of slugs) {
      await page.goto(`http://127.0.0.1:${adminPort}/admin/${slug}/`);
      await page.waitForSelector('#app:not([hidden])');
      assert.equal(await page.getAttribute(`#nav a[data-tab="${slug}"]`, 'aria-current'), 'page', slug);
      assert.equal(await page.locator('#nav a[aria-current="page"]').count(), 1, slug + ': one current page');
      assert.equal(await page.locator(`section[data-tab="${slug}"]`).count(), 1, slug + ': its own section');
      assert.equal(await page.locator('section[data-tab]').count(), 1, slug + ': and no other');
      for (const id of ['#logout', '#banners', '#page-error', '#login', '#setup']) assert.equal(await page.locator(id).count(), 1, slug + ' ' + id);
      assert.equal(await page.locator('#flash, #offline, #rejected, #chain-alert, #alerts-banner, #unsealed-banner, #sealed-alerts').count(), 0, slug + ': the old flash banners are gone');
      assert.match(await page.title(), / · Askbar admin$/);
    }
    await openPage('providers');
    await openPage('people');
    await page.goBack();
    await page.waitForURL('**/admin/providers/');
    await page.waitForSelector('#app:not([hidden])');
    assert.equal(await page.getAttribute('#nav a[data-tab="providers"]', 'aria-current'), 'page', 'Back returns to the page before');
    await page.goto(`http://127.0.0.1:${adminPort}/admin`);
    assert.match(page.url(), /\/admin\/home\/$/, '/admin goes to Home, the landing page');
    assert.equal(await page.getAttribute('#nav a[data-tab]', 'data-tab'), 'home', 'Home is the first menu item');
    assert.equal(await page.locator('#nav a[data-tab="home"] svg').count(), 1, 'Home has its icon');
    const missing = await page.goto(`http://127.0.0.1:${adminPort}/admin/nothing-here/`);
    assert.equal(missing.status(), 404, 'an unknown admin page is a 404');
    await page.goto(base);
  });
  // Plan step 5.4: the Settings overview. One row per section, each a link with one line of state from its own read.
  const OVERVIEW_SHAPES = {
    providers: /^\d+ providers?$/,
    people: /^\d+ (person|people), \d+ (client|clients)$/,
    services: /^(All \d+ services running|\d+ of \d+ services not running)$/,
    network: /^(No host name|\S.*)$/,
    security: /^(API key: \S.*|No API key)$/,
    backup: /^(Never|Last backup .+)$/,
    notifications: /^ntfy (on|off), email (on|off)$/,
    general: /^Kill switch (on|off)$/,
  };
  const overviewFilled = (slug) => page.waitForFunction((id) => {
    const t = document.getElementById(id).textContent.trim();
    return t !== '' && t !== 'Loading…';
  }, 'ov-' + slug);
  await step('settings overview: eight section rows in menu order, each a link with its state line filled', async () => {
    await openPage('settings');
    const rows = await page.$$eval('#settings-overview a.ov-row', (as) => as.map((a) => [a.getAttribute('href'), a.querySelector('.ov-name').textContent.trim()]));
    assert.deepEqual(rows, [
      ['/admin/providers/', 'Providers'], ['/admin/people/', 'People & clients'], ['/admin/services/', 'Services'], ['/admin/network/', 'Network'],
      ['/admin/security/', 'Security'], ['/admin/backup/', 'Backup'], ['/admin/notifications/', 'Notifications'], ['/admin/general/', 'General'],
    ]);
    for (const slug of Object.keys(OVERVIEW_SHAPES)) {
      await overviewFilled(slug);
      const text = (await page.textContent('#ov-' + slug)).trim();
      assert.match(text, OVERVIEW_SHAPES[slug], slug + ' shows its state: ' + text);
    }
    const sizes = await page.$$eval('#settings-overview a.ov-row', (as) => as.map((a) => a.getBoundingClientRect().height));
    assert.ok(sizes.every((h) => h >= (isPhone ? 44 : 24)), 'every row is a large target: ' + sizes.join(','));
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), true, 'no sideways scroll');
    for (const [href] of rows) {
      await page.goto(base + 'settings/');
      await page.waitForSelector('#app:not([hidden])');
      await page.click(`#settings-overview a[href="${href}"]`);
      await page.waitForURL('**' + href + '**');
      await page.waitForSelector('#app:not([hidden])');
      assert.equal(await page.getAttribute(`#nav a[data-tab="${href.split('/')[2]}"]`, 'aria-current'), 'page', href + ' opens its page');
    }
  });
  await step('settings overview: a failed read shows Unavailable on its row only, and the row fills again when the read works', async () => {
    await page.goto(base + 'settings/');
    await page.waitForSelector('#app:not([hidden])');
    const seen = errors.length;
    await page.route('**/api/admin/v1/network', (route) => route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"internal","detail":"faked by the test"}' }));
    try {
      await page.reload();
      await page.waitForSelector('#app:not([hidden])');
      await page.waitForFunction(() => document.getElementById('ov-network').textContent === 'Unavailable');
      for (const slug of Object.keys(OVERVIEW_SHAPES).filter((s) => s !== 'network')) {
        await overviewFilled(slug);
        assert.match((await page.textContent('#ov-' + slug)).trim(), OVERVIEW_SHAPES[slug], slug + ' still fills');
      }
    } finally {
      await page.unroute('**/api/admin/v1/network');
    }
    // The 500 is the fault this step made: its console line is not a page error.
    for (let i = errors.length - 1; i >= seen; i--) if (/status of 500/.test(errors[i])) errors.splice(i, 1);
    await page.reload();
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForFunction(() => { const t = document.getElementById('ov-network').textContent.trim(); return t !== '' && t !== 'Loading…' && t !== 'Unavailable'; });
    assert.match((await page.textContent('#ov-network')).trim(), OVERVIEW_SHAPES.network, 'the network row fills again');
  });
  await step('settings overview: the menu heading opens it and marks it current there; no sub-item repeats it', async () => {
    await openPage('providers');
    assert.equal(await page.locator('#nav .sm-subs a[data-tab="settings"]').count(), 0, 'the overview is not a sub-item');
    assert.equal(await page.getAttribute('#nav a[data-tab="settings"]', 'aria-current'), null, 'the heading is not current on another Settings page');
    assert.match(await page.getAttribute('#nav a[data-tab="settings"]', 'class'), /sm-parent/, 'the Settings heading is bold under Settings');
    await openPage('settings');
    assert.equal(await page.getAttribute('#nav a[data-tab="settings"]', 'aria-current'), 'page', 'the heading is current on the overview');
    assert.equal(await page.locator('#nav a[aria-current="page"]').count(), 1, 'one current item');
    const subs = await page.evaluate(() => [...document.querySelector('#nav a[data-tab="settings"]').nextElementSibling.querySelectorAll('a')].map((a) => a.dataset.tab));
    assert.deepEqual(subs, ['providers', 'people', 'services', 'network', 'security', 'backup', 'notifications', 'general']);
    assert.equal(await page.locator('#settings-overview').count(), 1, 'the overview is on the page');
  });

  await step('side menu: every page once, the current page and its group marked, the health line, licence and source from the constants', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    await page.goto(base + 'audit/sealed/');
    await page.waitForSelector('#app:not([hidden])');
    const menu = await page.evaluate(() => {
      const weight = (el) => getComputedStyle(el).fontWeight;
      const current = document.querySelector('#nav a[aria-current="page"]');
      const parent = document.querySelector('#nav .sm-parent');
      return {
        current: current.dataset.tab,
        currentBold: weight(current),
        parent: parent.textContent.trim(),
        parentBold: weight(parent),
        others: [...document.querySelectorAll('#nav > a.sm-item:not(.sm-parent)')].map((a) => weight(a)),
        width: document.getElementById('side-menu').getBoundingClientRect().width,
        licence: [document.querySelector('.sm-lic-link').textContent, document.querySelector('.sm-lic-link').href],
        source: document.querySelector('.sm-lic a[aria-label="Source code"]').href,
        footer: getComputedStyle(document.querySelector('.footer')).display,
      };
    });
    assert.equal(menu.current, 'audit/sealed');
    assert.equal(menu.currentBold, '600');
    assert.deepEqual([menu.parent, menu.parentBold], ['Audit', '600'], 'the Audit heading is bold on its pages');
    assert.ok(menu.others.every((w) => w === '400'), 'no other heading is bold: ' + menu.others);
    if (!isPhone) assert.equal(menu.width, 216, 'the menu is --rail-w wide');
    assert.deepEqual(menu.licence, [K.LICENSE, K.LICENSE_URL]);
    assert.equal(menu.source, K.SOURCE_URL);
    assert.equal(menu.footer, 'none', 'signed in, the menu carries the licence and the footer is hidden');
    await page.waitForFunction(() => /services (running|not running)$/.test(document.querySelector('#menu-health-status .status-word').textContent));
    assert.match(await page.getAttribute('#menu-health-status', 'class'), /status-(ok|warning)/);
    assert.equal(await page.getAttribute('#menu-health', 'href'), '/admin/services/');
    // The icon buttons show their names on keyboard focus, not only on hover (rule 42).
    await openMenu();
    await page.focus('#nav a[data-tab="reference"]');
    await page.keyboard.press('Tab');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'logout', 'Sign out follows the menu in the Tab order');
    await page.waitForSelector('#menu-tip:not([hidden])');
    assert.equal(await page.innerText('#menu-tip'), 'Sign out');
    await page.keyboard.press('Escape');
    assert.equal(await page.isHidden('#menu-tip'), true, 'Esc hides the tooltip');
    if (isPhone) await page.keyboard.press('Escape'); // and a second Esc closes the phone menu
    await page.goto(base);
    await page.waitForSelector('#app:not([hidden])');
  });
  await step(isPhone ? 'side menu: a phone has no collapse button, and a collapsed choice does not shrink its menu' : 'side menu: collapse shrinks it to icons, the choice survives a reload, names show on focus, and a blocked store leaves it working', async () => {
    if (isPhone) {
      await openMenu();
      assert.equal(await page.isHidden('#menu-collapse'), true, 'no collapse button on a phone');
      const before = await page.evaluate(() => document.getElementById('side-menu').getBoundingClientRect().width);
      await page.evaluate(() => document.documentElement.classList.add('menu-collapsed'));
      assert.equal(await page.evaluate(() => document.getElementById('side-menu').getBoundingClientRect().width), before);
      await page.evaluate(() => document.documentElement.classList.remove('menu-collapsed'));
      await page.keyboard.press('Escape');
      return;
    }
    const menuWidth = () => page.evaluate(() => document.getElementById('side-menu').getBoundingClientRect().width);
    const full = await menuWidth();
    const mainLeft = () => page.evaluate(() => document.querySelector('#main').getBoundingClientRect().left);
    const leftBefore = await mainLeft();
    await page.click('#menu-collapse');
    assert.equal(await page.getAttribute('#menu-collapse', 'aria-expanded'), 'false');
    assert.equal(await page.getAttribute('#menu-collapse', 'aria-label'), 'Expand menu');
    const collapsedW = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--rail-w-collapsed')));
    await page.waitForFunction((w) => document.getElementById('side-menu').getBoundingClientRect().width === w, collapsedW);
    assert.ok(await mainLeft() < leftBefore, 'the page gains the width');
    assert.equal(await page.evaluate(() => document.querySelector('#nav a[data-tab="providers"]').textContent.trim()), 'Providers', 'the name stays for screen readers');
    await page.reload();
    await page.waitForSelector('#app:not([hidden])');
    assert.equal(await page.getAttribute('#menu-collapse', 'aria-expanded'), 'false', 'still collapsed after a reload');
    assert.equal(await menuWidth(), collapsedW);
    // Keyboard focus on a link shows its name beside it.
    await page.focus('#nav a[data-tab="providers"]');
    await page.keyboard.press('Tab');
    await page.keyboard.press('Shift+Tab');
    await page.waitForSelector('#menu-tip:not([hidden])');
    assert.equal(await page.innerText('#menu-tip'), 'Providers');
    assert.ok(await page.evaluate(() => document.getElementById('menu-tip').getBoundingClientRect().left >= document.getElementById('side-menu').getBoundingClientRect().right), 'beside the menu, not clipped by it');
    await page.click('#menu-collapse');
    assert.equal(await page.getAttribute('#menu-collapse', 'aria-expanded'), 'true');
    await page.waitForFunction((w) => document.getElementById('side-menu').getBoundingClientRect().width === w, full);
    // A browser that refuses localStorage still gets a working, expanded menu.
    const blocked = await browser.newContext({ viewport, storageState: await ctx.storageState() });
    await blocked.addInitScript(() => {
      const no = () => { throw new Error('storage blocked'); };
      Storage.prototype.getItem = no;
      Storage.prototype.setItem = no;
    });
    const p = await blocked.newPage();
    const errs = [];
    p.on('pageerror', (e) => errs.push(e.message));
    try {
      await p.goto(base + 'providers/');
      await p.waitForSelector('#app:not([hidden])');
      assert.equal(await p.getAttribute('#menu-collapse', 'aria-expanded'), 'true');
      await p.click('#menu-collapse');
      assert.equal(await p.getAttribute('#menu-collapse', 'aria-expanded'), 'false', 'it still collapses, for this page');
      assert.deepEqual(errs, []);
    } finally {
      await blocked.close();
    }
  });
  await step(isPhone ? 'phone menu: the Menu button opens every page full screen, Tab stays inside, Esc closes it and returns focus' : 'phone menu: no Menu button or phone bar on a desktop', async () => {
    if (!isPhone) {
      assert.equal(await page.isHidden('#menu-open'), true);
      assert.equal(await page.isVisible('#side-menu'), true);
      return;
    }
    await page.goto(base + 'network/');
    await page.waitForSelector('#app:not([hidden])');
    assert.equal(await page.isHidden('#side-menu'), true, 'the menu is closed at first');
    const box = await page.locator('#menu-open').boundingBox();
    assert.ok(box.height >= 44 && box.width >= 44, 'the Menu button is a 44 px target: ' + JSON.stringify(box));
    await page.click('#menu-open');
    assert.equal(await page.getAttribute('#menu-open', 'aria-expanded'), 'true');
    assert.equal(await page.innerText('#menu-open'), 'Close');
    const r = await page.evaluate(() => {
      const m = document.getElementById('side-menu').getBoundingClientRect();
      return {
        full: m.left === 0 && Math.round(m.right) === document.documentElement.clientWidth && Math.round(m.bottom) === innerHeight,
        visible: [...document.querySelectorAll('#nav a[data-tab]')].filter((a) => a.getClientRects().length > 0).map((a) => a.dataset.tab),
        inert: document.querySelector('#main').inert,
        inside: document.getElementById('side-menu').contains(document.activeElement),
      };
    });
    const slugs = await page.$$eval('#nav a[data-tab]', (as) => as.map((a) => a.dataset.tab));
    assert.deepEqual(r, { full: true, visible: slugs, inert: true, inside: true });
    // Tab goes round the menu and its Close button, never into the page behind.
    for (let i = 0; i < slugs.length + 12; i++) {
      await page.keyboard.press('Tab');
      assert.ok(await page.evaluate(() => document.getElementById('side-menu').contains(document.activeElement) || document.activeElement.id === 'menu-open'), 'Tab ' + i + ' stays in the menu');
    }
    await page.keyboard.press('Escape');
    assert.equal(await page.isHidden('#side-menu'), true);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'menu-open', 'focus goes back to the Menu button');
    assert.equal(await page.evaluate(() => document.querySelector('#main').inert), false);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), true, 'no sideways scroll');
    // A page from the menu.
    await openPage('security');
    assert.equal(await page.isHidden('#side-menu'), true, 'the new page opens with the menu closed');
    await page.goto(base);
    await page.waitForSelector('#app:not([hidden])');
  });
  await step('status bar: 48 px and sticky, the breadcrumb names the page, the budget is greyed, the kill switch is a word, a focused field stays clear of it', async () => {
    const K = await page.evaluate(() => window.ui.ready);
    await page.goto(base + 'audit/sealed/');
    await page.waitForSelector('#app:not([hidden])');
    const crumb = () => page.evaluate(() => [...document.querySelectorAll('#status-bar .crumb li')].map((li) => [li.textContent, li.getAttribute('aria-current')]));
    assert.deepEqual(await crumb(), [['Audit', null], ['Sealed access', 'page']], 'a grouped page shows group > page');
    await page.goto(base + 'reference/');
    await page.waitForSelector('#app:not([hidden])');
    assert.deepEqual(await crumb(), [['Reference', 'page']], 'a top-level page shows just its label');
    if (isPhone) {
      // On a phone the group is left out, so the label is whole: the visible text is exactly the page name, not clipped.
      await page.goto(base + 'network/');
      await page.waitForSelector('#app:not([hidden])');
      const visible = await page.evaluate(() => {
        const items = [...document.querySelectorAll('#status-bar .crumb li')].filter((li) => getComputedStyle(li).display !== 'none');
        const cur = document.querySelector('#status-bar .crumb li[aria-current="page"]');
        return { text: items.map((li) => li.textContent.trim()).join(' '), clipped: cur.scrollWidth > cur.clientWidth };
      });
      assert.deepEqual(visible, { text: 'Network', clipped: false }, 'on a phone the breadcrumb shows only the page label, not clipped');
    }
    assert.equal(await page.locator('#status-bar [aria-current="page"]').count(), 1);
    assert.equal(await page.locator('nav[aria-label="Breadcrumb"] ol').count(), 1);
    await page.goto(base + 'general/');
    await page.waitForSelector('#app:not([hidden])');
    // Budget: one greyed item that is aria-disabled, never `disabled`; kill switch: a word with a symbol, once.
    const facts = await page.evaluate(() => ({
      na: document.querySelectorAll('#status-bar .na[aria-disabled="true"]').length,
      budgetDisabledAttr: document.querySelectorAll('#status-bar [disabled]').length,
      budgetTip: document.querySelector('#status-bar .na').dataset.tip,
      kill: document.querySelectorAll('#sb-kill').length,
      killFields: document.querySelectorAll('#s-kill-switch').length,
    }));
    assert.deepEqual(facts, { na: 1, budgetDisabledAttr: 0, budgetTip: 'Not available yet: the server has no family budget setting', kill: 1, killFields: 1 });
    await page.waitForFunction(() => /^Kill switch: (on|off)$/.test(document.querySelector('#sb-kill .status-word').textContent));
    assert.match(await page.getAttribute('#sb-kill', 'class'), /status-(ok|error)/);
    assert.equal(await page.evaluate(() => document.querySelector('#sb-kill .status-sym').getAttribute('aria-hidden')), 'true');
    // Height is --status-h and the bar stays put while the page scrolls.
    const bar = () => page.evaluate(() => {
      const r = document.getElementById('status-bar').getBoundingClientRect();
      return { top: Math.round(r.top), height: r.height, statusH: parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--status-h')), bottom: r.bottom, wide: document.documentElement.scrollWidth > document.documentElement.clientWidth };
    });
    const before = await bar();
    assert.equal(before.height, before.statusH, 'the bar is --status-h high');
    assert.equal(before.wide, false, 'no sideways scroll');
    await page.evaluate(() => { document.querySelector('#main').append(Object.assign(document.createElement('div'), { id: 't-tall', textContent: 'x' })); document.getElementById('t-tall').style.height = '3000px'; });
    await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
    const after = await bar();
    assert.equal(after.top, before.top, 'the bar does not move while scrolling');
    assert.equal(after.height, before.statusH);
    // A field focused from far down scrolls into view below the bar, not under it.
    const input = await page.evaluate(() => {
      const i = document.querySelector('#main input:not([type=hidden]):not([type=checkbox]):not([type=radio])');
      i.focus();
      const bottom = document.getElementById('status-bar').getBoundingClientRect().bottom;
      return { top: i.getBoundingClientRect().top, bottom, scrolled: window.scrollY };
    });
    assert.ok(input.top >= input.bottom, 'the focused field starts below the bar: ' + JSON.stringify(input));
    await page.evaluate(() => document.getElementById('t-tall').remove());
    // The status poll still runs every STATUS_EVERY_SECS.
    let polls = 0;
    const count = (r) => { if (r.url().endsWith('/api/admin/v1/status')) polls++; };
    page.on('request', count);
    await page.waitForTimeout(K.STATUS_EVERY_SECS * 2 * 1000 + 1500);
    page.off('request', count);
    assert.ok(polls >= 2 && polls <= 3, 'status polled every STATUS_EVERY_SECS: ' + polls);
    await openPage('providers');
  });
  await step('pages: an audit request opens directly from its address, in the docked panel, and the address follows it', async () => {
    const list = await page.evaluate(async () => (await fetch('/api/admin/v1/audit/traces', { credentials: 'same-origin' })).json());
    const id = list.traces[0].trace_id;
    await page.goto(`http://127.0.0.1:${adminPort}/admin/audit/?view=trace&id=${encodeURIComponent(id)}`);
    // The old address opens Model calls, and its id opens the trace in the docked panel (the views are gone, plan step 6.1).
    await page.waitForSelector('#trace-content:not([hidden])');
    assert.match(page.url(), /\/admin\/audit\/calls\/\?/, 'the old address opened Model calls');
    assert.equal(await page.innerText('#trace-id'), id);
    assert.equal(new URL(page.url()).searchParams.get('id'), id, 'the address keeps the request');
    await page.keyboard.press('Escape');
    await page.waitForSelector('#trace-panel', { state: 'hidden' });
    await page.waitForFunction(() => !location.search.includes('id='));
    await openPage('providers');
  });
  await step('pages: a static file is kept and answers 304 when unchanged; pages are never stored', async () => {
    const first = await page.evaluate(async () => {
      const r = await fetch('/admin/app.js');
      return { status: r.status, etag: r.headers.get('etag'), cache: r.headers.get('cache-control') };
    });
    assert.equal(first.status, 200);
    assert.equal(first.cache, 'no-cache');
    assert.match(first.etag, /^"[0-9a-f]{16}"$/);
    const again = await page.evaluate(async (etag) => (await fetch('/admin/app.js', { headers: { 'If-None-Match': etag } })).status, first.etag);
    assert.equal(again, 304);
    const pageCache = await page.evaluate(async () => (await fetch('/admin/providers/')).headers.get('cache-control'));
    assert.equal(pageCache, 'no-store');
  });
  await step('sign out returns to the login form', async () => {
    await openMenu();
    await page.click('#logout');
    await page.waitForSelector('#login:not([hidden])');
    await page.reload();
    await page.waitForSelector('#login:not([hidden])');
    await page.fill('#password', PASSWORD);
    await page.click('#login-form button');
    await page.waitForSelector('#password-error:not([hidden])');
    await page.fill('#password', 'brand new password 1');
    await page.click('#login-form button');
    await page.waitForSelector('#app:not([hidden])');
  });
  await step('with the server stopped the admin still loads and signs in, and says the server is offline', async () => {
    serverProc.kill();
    await new Promise((r) => serverProc.once('exit', r));
    await page.reload();
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#banners [data-banner="offline"]', { timeout: 15000 });
    assert.match(await page.innerText('#banners [data-banner="offline"]'), /Server offline/);
    // Sign out and in again with the server away: the admin has its own login.
    await openMenu();
    await page.click('#logout');
    await page.waitForSelector('#login:not([hidden])');
    await page.fill('#password', 'brand new password 1');
    await page.click('#login-form button');
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#banners [data-banner="offline"]');
  });
  await step('audit: with the server away the trail still reads from the auditor, and each Audit page is there', async () => {
    await openPage('audit/calls');
    await page.waitForSelector('#calls-rows tr[data-trace]');
    assert.equal(await page.isHidden('#calls-error'), true, 'the auditor is up, so there is nothing to explain');
    assert.ok(Number((await page.innerText('#kpi-replies')).replace(/,/g, '')) >= 1, 'the figures read from the auditor');
    await openPage('audit/sealed');
    await page.waitForSelector('#audit-unseal-form:not([hidden])');
    assert.equal(await page.isHidden('#audit-error'), true, 'the auditor is up, so there is nothing to explain');
    assert.match(await page.innerText('#tab-audit-sealed'), /Who opened sealed content/);
    await openPage('audit/routing');
    await page.waitForSelector('#audit-routing-overall-empty:not([hidden]), #audit-routing-table:not([hidden])');
    assert.equal(await page.isHidden('#audit-error'), true, 'the auditor is up, so there is nothing to explain');
  });
  await step('a change while the server is away is refused with a clear message and saves nothing', async () => {
    await openPage('providers');
    await page.click('#add-provider');
    await page.waitForSelector('#provider-panel:not([hidden])');
    await page.fill('#p-name', 'while-away');
    await page.fill('#p-model', 'm');
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('#provider-error:not([hidden])');
    assert.match(await page.innerText('#provider-error'), /offline/i);
    await page.click('#provider-cancel');
    await page.click('dialog[open] [data-confirm-action]');
    await panelClosed();
    assert.equal(await page.locator('tr[data-name="while-away"]').count(), 0);
    // What the admin owns can still be read.
    assert.equal(await page.locator('tr[data-name="mock"]').count(), 1);
  });
  await step('when the server is back the banner goes away and changes work again', async () => {
    serverProc = startServer();
    await waitForPort(api);
    await page.waitForSelector('#banners [data-banner="offline"]', { state: 'detached', timeout: 30000 });
    await page.click('#add-provider');
    await page.waitForSelector('#provider-panel:not([hidden])');
    await page.fill('#p-name', 'after-return');
    await page.fill('#p-model', 'm');
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('tr[data-name="after-return"]');
    await panelClosed();
  });
  await step('a token the server stops accepting gets its own banner with the command, and it clears when the server accepts it again', async () => {
    const good = fs.readFileSync(hashFile, 'utf8');
    // The server's side is rotated: its hash file now holds the hash of some other token (read per request).
    fs.chmodSync(path.dirname(hashFile), 0o700);
    fs.rmSync(hashFile);
    fs.writeFileSync(hashFile, 'f'.repeat(64) + '\n');
    await page.waitForSelector('#banners [data-banner="rejected"]', { timeout: 20000 });
    const banner = await page.innerText('#banners [data-banner="rejected"]');
    assert.match(banner, /rejected this admin's token/);
    assert.match(banner, /bootstrap rotate admin-server/);
    assert.match(banner, /docker compose restart server admin/);
    assert.deepEqual(await page.$$eval('#banners [data-banner="rejected"] code', (c) => c.map((x) => x.textContent)), ['docker compose run --rm bootstrap rotate admin-server', 'docker compose restart server admin'], 'the commands are code, built as text');
    assert.equal(await page.locator('#banners [data-banner="rejected"] button').count(), 0, 'a banner has no close button');
    assert.equal(await page.isVisible('#banners [data-banner="offline"]'), false, 'one message, the useful one');
    fs.writeFileSync(hashFile, good);
    await page.waitForSelector('#banners [data-banner="rejected"]', { state: 'detached', timeout: 20000 });
  });
  await step('audit chain: a rollback, a rewrite and an edit each get a banner with its sentence, and it clears', async () => {
    const real = await page.evaluate(async () => (await fetch('/api/admin/v1/status')).json());
    assert.equal(real.audit_chain.state === 'ok' || real.audit_chain.state === 'unknown', true, 'nothing is wrong with the real chain: ' + JSON.stringify(real.audit_chain));
    const withChain = async (audit_chain) => {
      await page.unroute('**/api/admin/v1/status');
      await page.route('**/api/admin/v1/status', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ ...real, audit_chain }) }));
      await page.reload();
      await page.waitForSelector('#app:not([hidden])');
    };
    await withChain({ state: 'rollback', detail: 'The audit trail is shorter than it was: <b>x</b>' });
    await page.waitForSelector('#banners [data-banner="chain"]');
    assert.equal(await page.innerText('#banners [data-banner="chain"]'), 'The audit trail is shorter than it was: <b>x</b>', 'the sentence is shown as text');
    assert.equal(await page.locator('#banners [data-banner="chain"] b').count(), 0);
    await withChain({ state: 'rewritten', detail: 'The audit trail was rewritten.' });
    await page.waitForSelector('#banners [data-banner="chain"]');
    assert.match(await page.innerText('#banners [data-banner="chain"]'), /rewritten/);
    await withChain({ state: 'edited', detail: 'Records in the audit trail do not match their entries.' });
    await page.waitForSelector('#banners [data-banner="chain"]');
    await withChain({ state: 'ok', detail: '' });
    await page.waitForSelector('#banners [data-banner="chain"]', { state: 'detached' });
    await page.unroute('**/api/admin/v1/status');
  });
  await step('a banner is announced assertively, has no close button, and is text only', async () => {
    await page.evaluate(() => window.ui.banner('probe', 'error', 'Probe <i>banner</i>', { action: { label: 'Fix it', onclick: () => {} } }));
    await page.waitForFunction(() => document.getElementById('announcer-urgent').textContent === 'Probe <i>banner</i>');
    assert.equal(await page.getAttribute('#announcer-urgent', 'aria-live'), 'assertive');
    assert.equal(await page.locator('#banners [data-banner="probe"] i').count(), 0);
    assert.deepEqual(await page.$$eval('#banners [data-banner="probe"] button', (b) => b.map((x) => x.textContent)), ['Fix it'], 'one fix action and no close button');
    await page.evaluate(() => window.ui.banner('probe', 'error', null));
    assert.equal(await page.locator('#banners [data-banner="probe"]').count(), 0, 'cleared by the condition ending');
  });
  await step('no JavaScript errors', async () => { assert.deepEqual(errors, []); });

  } finally {
    if (browser) await browser.close().catch(() => {});
    cleanup();
  }
}

// The viewports run one after the other, each on its own stack. A crash still cleans up (the finally above) and fails the run.
(async () => {
  const started = Date.now();
  for (const vp of VIEWPORTS) {
    try { await runViewport(vp); }
    catch (e) { failures++; console.error('FAIL [' + vpLabel + '] the run stopped: ' + (e && e.stack || e)); }
  }
  for (const [w, c] of Object.entries(counts)) console.log(`[${w}] ${c.ok} ok, ${c.fail} failed`);
  console.log(`${Math.round((Date.now() - started) / 1000)} s`);
  console.log(failures ? `\n${failures} step(s) failed` : '\nall admin steps passed');
  process.exit(failures ? 1 : 0);
})();
