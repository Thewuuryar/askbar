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
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of (4\d\d|502)/.test(m.text())) errors.push(m.text()); });

  // Opens the Settings tab and waits until its values have been read from the server. Waiting for a field to hold a
  // value is not enough when the test typed that same value: a read that lands later overwrites what the test types next.
  async function openSettings() {
    await openPage('providers');
    const reread = page.waitForResponse((r) => r.url().endsWith('/api/admin/v1/settings') && r.request().method() === 'GET');
    await openPage('settings');
    await reread;
  }

  await step('first run: sign in with the published login (ADR 0007)', async () => {
    await page.goto(base);
    await page.waitForSelector('#login:not([hidden])');
    assert.equal(await page.inputValue('#username'), 'admin');
    await page.fill('#username', 'root');
    await page.fill('#password', 'admin');
    await page.click('#login-form button');
    await page.waitForSelector('#login-error:not([hidden])');
    assert.match(await page.innerText('#login-error'), /Wrong username or password/, 'a wrong name gets the same answer as a wrong password');
    await page.fill('#username', 'admin');
    await page.fill('#password', 'admin');
    await page.click('#login-form button');
    await page.waitForSelector('#setup-password-form:not([hidden])');
    assert.equal(await page.isHidden('#app'), true, 'nothing else opens before the password is changed');
  });
  const AUDIT_PASS = 'correct horse battery staple 42';
const BACKUP_PASS = 'e2e backup passphrase, its own 1';
const BACKUP_PASS_2 = 'e2e backup passphrase, its own 2';
  await step('setup: the published password must be changed, to something else, then the audit passphrase step opens', async () => {
    await page.fill('#setup-current', 'admin');
    await page.fill('#setup-password', PASSWORD);
    await page.fill('#setup-password2', PASSWORD + 'x');
    await page.click('#setup-password-form button[type=submit]');
    await page.waitForSelector('#setup-password-error:not([hidden])');
    assert.match(await page.innerText('#setup-password-error'), /do not match/);
    // The published password (`admin`) as the new one is refused (by the server too, not only the form).
    await page.evaluate(() => { document.getElementById('setup-password-form').noValidate = true; });
    await page.fill('#setup-password', 'admin');
    await page.fill('#setup-password2', 'admin');
    await page.click('#setup-password-form button[type=submit]');
    await page.waitForFunction(() => /at least 10/.test(document.getElementById('setup-password-error').textContent));
    await page.fill('#setup-password', PASSWORD);
    await page.fill('#setup-password2', PASSWORD);
    await page.click('#setup-password-form button[type=submit]');
    await page.waitForSelector('#setup-audit-form:not([hidden])');
    assert.equal(await page.isHidden('#app'), true, 'setup does not finish at the password');
    assert.equal(await page.isHidden('#setup-password-form'), true);
    assert.equal(await page.getAttribute('#setup-audit', 'type'), 'password');
    assert.match(await page.innerText('#setup-audit-form'), /cannot be recovered/);
    // A new install can be set up from a backup instead: the link opens the Restore page without finishing the setup screens.
    await page.click('#setup-restore-link');
    await page.waitForURL('**/admin/restore/**');
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#restore-upload-form:not([hidden])');
    assert.equal(await page.isHidden('#setup'), true);
    await page.goto(base);
    await page.waitForSelector('#setup-audit-form:not([hidden])');
  });
  await step('setup: the audit passphrase must match, be 20+ characters and be confirmed as stored', async () => {
    await page.evaluate(() => document.getElementById('setup-audit-form').noValidate = true);
    await page.fill('#setup-audit', AUDIT_PASS);
    await page.fill('#setup-audit2', AUDIT_PASS + 'x');
    await page.check('#setup-audit-stored');
    await page.click('#setup-audit-form button[type=submit]');
    assert.match(await page.innerText('#setup-audit-error'), /do not match/);
    await page.fill('#setup-audit', 'too short');
    await page.fill('#setup-audit2', 'too short');
    await page.click('#setup-audit-form button[type=submit]');
    assert.match(await page.innerText('#setup-audit-error'), /at least 20/);
    // Without the confirmation nothing is created, even with a good passphrase.
    await page.fill('#setup-audit', AUDIT_PASS);
    await page.fill('#setup-audit2', AUDIT_PASS);
    await page.uncheck('#setup-audit-stored');
    await page.click('#setup-audit-form button[type=submit]');
    assert.match(await page.innerText('#setup-audit-error'), /stored the passphrase/);
    assert.equal(await page.isHidden('#app'), true);
    const st = await (await fetch(`http://127.0.0.1:${adminPort}/api/admin/v1/setup`, { headers: { 'x-askbar-admin': '1' } })).json();
    assert.equal(st.audit_key_set, false, 'no key yet');
  });
  await step('setup: saving shows the key id, then Continue goes on to the backup passphrase', async () => {
    await page.check('#setup-audit-stored');
    await page.click('#setup-audit-form button[type=submit]');
    await page.waitForSelector('#setup-audit-done:not([hidden])');
    const keyId = await page.innerText('#setup-audit-keyid');
    assert.match(keyId, /^[0-9a-f]{16}$/);
    assert.equal(await page.inputValue('#setup-audit').catch(() => ''), '', 'the passphrase field is cleared');
    assert.equal((await page.content()).includes(AUDIT_PASS), false, 'the passphrase is not left in the page');
    await page.click('#setup-audit-continue');
    await page.waitForSelector('#setup-backup-form:not([hidden])');
    assert.equal(await page.isHidden('#app'), true, 'setup does not finish at the audit key');
    assert.equal(await page.getAttribute('#setup-backup', 'type'), 'password');
    assert.match(await page.innerText('#setup-backup-form'), /cannot be recovered/);
  });
  await step('setup: the backup passphrase must match, be 20+ characters, differ from the audit passphrase and be confirmed as stored (ADR 0013)', async () => {
    await page.evaluate(() => document.getElementById('setup-backup-form').noValidate = true);
    await page.fill('#setup-backup', BACKUP_PASS);
    await page.fill('#setup-backup2', BACKUP_PASS + 'x');
    await page.check('#setup-backup-stored');
    await page.click('#setup-backup-form button[type=submit]');
    assert.match(await page.innerText('#setup-backup-error'), /do not match/);
    await page.fill('#setup-backup', 'too short');
    await page.fill('#setup-backup2', 'too short');
    await page.click('#setup-backup-form button[type=submit]');
    assert.match(await page.innerText('#setup-backup-error'), /at least 20/);
    // The audit passphrase is refused by the admin, which knows it can open an audit key.
    await page.fill('#setup-backup', AUDIT_PASS);
    await page.fill('#setup-backup2', AUDIT_PASS);
    await page.click('#setup-backup-form button[type=submit]');
    await page.waitForFunction(() => /different from the audit passphrase/.test(document.getElementById('setup-backup-error').textContent));
    // Without the confirmation nothing is created, even with a good passphrase.
    await page.fill('#setup-backup', BACKUP_PASS);
    await page.fill('#setup-backup2', BACKUP_PASS);
    await page.uncheck('#setup-backup-stored');
    await page.click('#setup-backup-form button[type=submit]');
    assert.match(await page.innerText('#setup-backup-error'), /stored the passphrase/);
    assert.equal(await page.isHidden('#app'), true);
    const st = await (await fetch(`http://127.0.0.1:${adminPort}/api/admin/v1/setup`, { headers: { 'x-askbar-admin': '1' } })).json();
    assert.equal(st.backup_key_set, false, 'no backup key yet');
  });
  await step('setup: saving shows the fingerprint and the next steps, then Continue opens the app', async () => {
    await page.check('#setup-backup-stored');
    await page.click('#setup-backup-form button[type=submit]');
    await page.waitForSelector('#setup-backup-done:not([hidden])');
    assert.match(await page.innerText('#setup-backup-fingerprint'), /^[0-9a-f]{4}(-[0-9a-f]{4}){3}$/);
    assert.equal(await page.inputValue('#setup-backup').catch(() => ''), '', 'the passphrase field is cleared');
    assert.equal(await page.inputValue('#setup-backup2').catch(() => ''), '');
    assert.equal((await page.content()).includes(BACKUP_PASS), false, 'the passphrase is not left in the page');
    assert.match(await page.innerText('#setup-next'), /Network[\s\S]*Providers[\s\S]*Clients/, 'the next steps, in order');
    await page.click('#setup-backup-continue');
    await page.waitForSelector('#app:not([hidden])');
    assert.equal(await page.isHidden('#setup'), true);
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
    await page.click('#logout');
    await page.waitForSelector('#login:not([hidden])');
    await page.fill('#password', 'nope');
    await page.click('#login-form button');
    await page.waitForSelector('#login-error:not([hidden])');
    assert.match(await page.innerText('#login-error'), /Wrong username or password/);
    await page.fill('#password', PASSWORD);
    await page.click('#login-form button');
    await page.waitForSelector('#app:not([hidden])');
  });

  await step('add the mock provider through the page', async () => {
    await page.click('#add-provider');
    await page.fill('#p-name', 'mock');
    await page.fill('#p-base-url', `http://127.0.0.1:${mockPort}/v1`);
    await page.fill('#p-model', 'mock-model');
    await page.fill('#p-key', SECRET_KEY);
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('.card[data-name="mock"]');
    assert.match(await page.innerText('.card[data-name="mock"]'), /key set/);
    assert.equal(await page.isHidden('#provider-form'), true);
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
    assert.equal(await page.locator('dialog').count(), 0, 'the box is removed once answered');

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

    await page.click('#t-copy');
    await page.waitForFunction(() => document.getElementById('t-copy').getAttribute('aria-label') === 'Copied');
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'askbar-admin-xyz');
    await page.evaluate(() => document.getElementById('t-misc').remove());
  });
  await step('approved models: none at first; pick two from the provider\'s list, choose the default, save, and it persists', async () => {
    assert.match(await page.innerText('#catalog-approved'), /No model is approved yet/);
    await page.selectOption('#catalog-provider', { label: 'mock' });
    await page.click('#catalog-load');
    await page.waitForSelector('#catalog-offered input[type=checkbox]');
    assert.deepEqual((await page.$$eval('#catalog-offered span', (s) => s.map((x) => x.textContent.trim()))), ['mock-model', 'mock-model-2']);
    await page.check('#catalog-offered label:nth-child(1) input');
    assert.match(await page.innerText('#catalog-approved'), /mock \/ mock-model \(default\)/, 'the first one approved becomes the default');
    await page.check('#catalog-offered label:nth-child(2) input');
    await page.check('#catalog-approved [data-model$="/mock-model-2"] input[type=radio]');
    assert.match(await page.innerText('#catalog-approved'), /mock \/ mock-model-2 \(default\)/);
    await page.click('#catalog-save');
    await page.waitForFunction(() => document.getElementById('catalog-status').textContent === 'Saved.');
    await page.reload();
    await page.waitForSelector('#catalog-approved [data-model]');
    assert.equal(await page.locator('#catalog-approved [data-model]').count(), 2);
    assert.match(await page.innerText('#catalog-approved'), /mock \/ mock-model-2 \(default\)/, 'the default persisted');
    // Removing the default leaves the next one as the default, never none.
    await page.click('#catalog-approved [data-model$="/mock-model-2"] button');
    assert.match(await page.innerText('#catalog-approved'), /mock \/ mock-model \(default\)/);
    await page.click('#catalog-discard');
    await page.waitForFunction(() => document.querySelectorAll('#catalog-approved [data-model]').length === 2);
    assert.equal(await page.locator('#catalog-approved [data-model]').count(), 2, 'discarding goes back to what is saved');
  });
  await step('Test sends a message through the provider and shows success', async () => {
    await page.click('.card[data-name="mock"] [data-action="test"]');
    await page.waitForFunction(() => /Success|Failed/.test(document.querySelector('.card[data-name="mock"] .test-result').textContent));
    assert.match(await page.innerText('.card[data-name="mock"] .test-result'), /^Success: Streaming works\. The first text came after \d+ ms\./);
    // The details are folded away until asked for, and are text: the request, the status and the timings, nothing else.
    assert.equal(await page.isVisible('.card[data-name="mock"] .test-details dl'), false);
    await page.click('.card[data-name="mock"] .test-details summary');
    const details = await page.innerText('.card[data-name="mock"] .test-details dl');
    assert.match(details, /Request\s+POST \/v1\/chat\/completions/);
    assert.match(details, /Status\s+200/);
    assert.match(details, /Time to first text\s+\d+ ms/);
    assert.equal(details.includes(SECRET_KEY), false);
  });
  await step('after a reload the key is not displayed anywhere', async () => {
    await page.reload();
    await page.waitForSelector('.card[data-name="mock"]');
    assert.equal((await page.content()).includes(SECRET_KEY), false);
    await page.click('.card[data-name="mock"] [data-action="edit"]');
    assert.equal(await page.inputValue('#p-key'), '');
    assert.match(await page.getAttribute('#p-key', 'placeholder'), /unchanged/);
    assert.equal(await page.getAttribute('#p-key', 'type'), 'password');
    const values = await page.$$eval('#provider-form input, #provider-form select', (els) => els.map((e) => e.value).join('|'));
    assert.equal(values.includes(SECRET_KEY), false);
  });
  await step('editing a provider with a saved key turns the model into a dropdown of what it offers', async () => {
    await page.waitForSelector('#p-model-list:not([hidden])');
    assert.equal(await page.isHidden('#p-model'), true);
    assert.deepEqual(await page.$$eval('#p-model-list option', (o) => o.map((x) => x.value)), ['mock-model', 'mock-model-2', '__manual__']);
    assert.equal(await page.inputValue('#p-model-list'), 'mock-model', 'the saved model is selected');
    assert.equal(await page.isHidden('#p-key'), false);
    assert.equal(await page.inputValue('#p-key'), '', 'the saved key was used by the server; the page never held it');
  });
  await step('"Enter a name myself" brings back the text box', async () => {
    await page.selectOption('#p-model-list', '__manual__');
    assert.equal(await page.isVisible('#p-model'), true);
    assert.equal(await page.isHidden('#p-model-list'), true);
    await page.click('#p-model-refresh');
    await page.waitForSelector('#p-model-list:not([hidden])');
  });
  await step('editing without a new key keeps the old one working', async () => {
    await page.selectOption('#p-model-list', 'mock-model-2');
    await page.click('#provider-form button[type=submit]');
    await page.waitForFunction(() => /mock-model-2/.test(document.querySelector('.card[data-name="mock"]')?.textContent || ''));
    await page.click('.card[data-name="mock"] [data-action="test"]');
    await page.waitForFunction(() => /Success|Failed/.test(document.querySelector('.card[data-name="mock"] .test-result').textContent));
    assert.match(await page.innerText('.card[data-name="mock"] .test-result'), /^Success:/);
  });
  await step('provider billing: set a subscription with prices, see it on the card, refuse a negative price', async () => {
    await page.click('.card[data-name="mock"] [data-action="edit"]');
    assert.equal(await page.inputValue('#p-billing'), 'metered', 'the default');
    assert.equal(await page.locator('#p-fallback').count(), 0, 'there is no provider fallback (ADR 0014)');
    await page.selectOption('#p-billing', 'subscription');
    await page.fill('#p-price-in', '2.5');
    await page.fill('#p-price-out', '10');
    await page.fill('#p-price-cached', '0.25');
    await page.click('#provider-form button[type=submit]');
    await page.waitForFunction(() => /subscription/.test(document.querySelector('.card[data-name="mock"]')?.textContent || ''));
    const card = await page.innerText('.card[data-name="mock"]');
    assert.doesNotMatch(card, /fallback/);
    assert.match(card, /in 2\.5, out 10, cached in 0\.25/);
    // It is kept: edit again, the values are back.
    await page.click('.card[data-name="mock"] [data-action="edit"]');
    assert.equal(await page.inputValue('#p-billing'), 'subscription');
    assert.equal(await page.inputValue('#p-price-out'), '10');
    // A negative price is refused with the server's rule, and nothing changes.
    await page.evaluate(() => document.getElementById('provider-form').noValidate = true);
    await page.fill('#p-price-in', '-1');
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('#provider-error:not([hidden])');
    assert.match(await page.innerText('#provider-error'), /zero or more/);
    // Put it back to an ordinary metered provider for the rest of the run.
    await page.fill('#p-price-in', '');
    await page.fill('#p-price-out', '');
    await page.fill('#p-price-cached', '');
    await page.selectOption('#p-billing', 'metered');
    await page.click('#provider-form button[type=submit]');
    await page.waitForFunction(() => !/subscription/.test(document.querySelector('.card[data-name="mock"]')?.textContent || ''));
  });
  await step('a provider that cannot be reached shows its error', async () => {
    const dead = await freePort();
    await page.click('#add-provider');
    await page.fill('#p-name', 'dead');
    await page.fill('#p-base-url', `http://127.0.0.1:${dead}/v1`);
    await page.fill('#p-model', 'm');
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('.card[data-name="dead"]');
    await page.click('.card[data-name="dead"] [data-action="test"]');
    await page.waitForFunction(() => /Failed/.test(document.querySelector('.card[data-name="dead"] .test-result').textContent));
    assert.match(await page.innerText('.card[data-name="dead"] .test-result'), /Could not reach the provider.s address/);
  });
  await step('a duplicate name is explained', async () => {
    await page.click('#add-provider');
    await page.fill('#p-name', 'dead');
    await page.fill('#p-model', 'm');
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('#provider-error:not([hidden])');
    assert.match(await page.innerText('#provider-error'), /already exists/);
    await page.click('#provider-cancel');
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
  await step('CLIProxyAPI: the accounts panel is on the Services tab, and says so when CLIProxyAPI is not answering', async () => {
    await openPage('services');
    await page.waitForSelector('#cpa-panel:not([hidden])');
    await page.waitForFunction(() => /not answering/.test(document.getElementById('cpa-message').textContent));
    await page.click('#cpa-connect-anthropic');
    await page.waitForFunction(() => /CLIProxyAPI did not answer/.test(document.getElementById('cpa-message').textContent));
    assert.equal(await page.isHidden('#cpa-login'), true, 'no login opens when CLIProxyAPI cannot start one');
    assert.equal(await page.locator('#add-cpa').count(), 0, 'the old preset button is gone');
    await openPage('providers');
  });
  await step('delete asks first, then removes the provider', async () => {
    page.once('dialog', (d) => d.dismiss());
    await page.click('.card[data-name="dead"] [data-action="delete"]');
    assert.equal(await page.locator('.card[data-name="dead"]').count(), 1);
    page.once('dialog', (d) => d.accept());
    await page.click('.card[data-name="dead"] [data-action="delete"]');
    await page.waitForFunction(() => !document.querySelector('.card[data-name="dead"]'));
  });
  await step('Services: the six services show whether they answer, and Restart asks first and then acts', async () => {
    // No controller (ADR 0005): each state comes from the service. Here the server, the gateway and the admin run;
    // Caddy and CLIProxyAPI do not.
    await openPage('services');
    await page.waitForSelector('#services .card[data-name="cpa"]');
    assert.deepEqual(await page.$$eval('#services .card', (c) => c.map((x) => x.dataset.name)), ['server', 'model-gateway', 'alerter', 'admin', 'caddy', 'cpa']);
    // The alerter runs here too (ADR 0055), so it joins the list between the gateway and the admin.
    assert.match(await page.innerText('#services .card[data-name="alerter"]'), /running/);
    // Its daily release check asked the stand-in GitHub, with a fixed User-Agent and nothing else about the install, and the card says so (ADR 0055).
    await page.waitForFunction(() => /Askbar is up to date \(/.test(document.querySelector('#services .card[data-name="alerter"]').innerText), null, { timeout: 15000 });
    const releaseRequests = JSON.parse(await new Promise((resolve, reject) => { http.get({ host: '127.0.0.1', port: releasesPort, path: '/_seen' }, (res) => { let b = ''; res.on('data', (c) => (b += c)); res.on('end', () => resolve(b)); }).on('error', reject); }));
    assert.ok(releaseRequests.length >= 1 && releaseRequests.every((r) => r.user_agent === 'askbar-release-check' && r.headers.every((h) => ['host', 'user-agent', 'accept'].includes(h))), JSON.stringify(releaseRequests));
    assert.deepEqual(await page.$$eval('#services .card[data-name="alerter"] button', (b) => b.map((x) => x.dataset.action)), ['restart']);
    assert.match(await page.innerText('#services .card[data-name="server"]'), /running/);
    // The gateway runs here too, and it has a Restart button like the rest.
    assert.match(await page.innerText('#services .card[data-name="model-gateway"]'), /running/);
    assert.deepEqual(await page.$$eval('#services .card[data-name="model-gateway"] button', (b) => b.map((x) => x.dataset.action)), ['restart']);
    assert.match(await page.innerText('#services .card[data-name="caddy"]'), /unreachable/);
    assert.deepEqual(await page.$$eval('#services .card[data-name="server"] button', (b) => b.map((x) => x.dataset.action)), ['restart'], 'only Restart: there is no start or stop');
    let asked = '';
    page.once('dialog', (d) => { asked = d.message(); d.dismiss(); });
    await page.click('#services .card[data-name="server"] [data-action="restart"]');
    assert.match(asked, /Replies being written end as interrupted/);
    await page.waitForFunction(() => /None yet/.test(document.getElementById('services-recent').textContent));
    // CLIProxyAPI is restarted through its supervisor (ADR 0037), so it has a Restart button too. (Not
    // clicked here: there is no supervisor in this test, and the admin would wait a minute for it.)
    assert.deepEqual(await page.$$eval('#services .card[data-name="cpa"] button', (b) => b.map((x) => x.dataset.action)), ['restart']);
    // Accepted: the attempt is made and recorded; Caddy is not running here, so it fails and says so.
    page.once('dialog', (d) => d.accept());
    await page.click('#services .card[data-name="caddy"] [data-action="restart"]');
    await page.waitForFunction(() => /restart caddy · failed/.test(document.getElementById('services-recent').textContent));
  });
  await step('clients see the approved models only with a token', async () => {
    // The API is the other listener; without a token it must refuse.
    const r = await fetch(`http://127.0.0.1:${api}/api/client/v1/models`, { headers: { 'x-askbar-client-version': '0.6.0' } });
    assert.equal(r.status, 401);
  });
  await step('clients: add a user, generate a pairing key for them, pair with it, then revoke the client', async () => {
    await openPage('clients');
    await page.waitForSelector('#pair-form');
    await page.waitForFunction(() => /No clients are paired yet/.test(document.getElementById('clients').textContent));
    // A fresh install has no users, so nothing can be paired yet (ADR 0008).
    await page.waitForFunction(() => /No users yet/.test(document.getElementById('users').textContent));
    assert.equal(await page.isDisabled('#pair-form button[type=submit]'), true);
    await page.fill('#u-name', 'Ann');
    await page.click('#user-form button[type=submit]');
    await page.waitForFunction(() => /Ann/.test(document.getElementById('users').textContent));
    assert.equal(await page.inputValue('#d-user').then((v) => v.length > 0), true, 'the new user is picked');
    assert.equal(await page.isDisabled('#pair-form button[type=submit]'), false);
    await page.fill('#d-name', 'Test laptop');
    await page.click('#pair-form button[type=submit]');
    await page.waitForSelector('#pairing-out:not([hidden])');
    const pairing = await page.inputValue('#pairing-string');
    assert.match(pairing, /^askbar-pair:/);
    const { url, code } = JSON.parse(Buffer.from(pairing.slice('askbar-pair:'.length), 'base64url').toString());
    assert.equal(url, 'http://localhost:' + api);
    const headers = { 'content-type': 'application/json', 'x-askbar-client-version': '0.6.0' };
    const paired = await fetch(`http://127.0.0.1:${api}/api/client/v1/pairings`, { method: 'POST', headers, body: JSON.stringify({ code }) });
    assert.equal(paired.status, 200);
    const { token } = await paired.json();
    const whoami = () => fetch(`http://127.0.0.1:${api}/api/client/v1/me`, { headers: { ...headers, authorization: 'Bearer ' + token } }).then((r) => r.status);
    assert.equal(await whoami(), 204);

    await openPage('providers');
    await openPage('clients');
    await page.waitForSelector('#clients .card');
    assert.match(await page.innerText('#clients .card'), /Test laptop[\s\S]*active[\s\S]*Ann · paired[\s\S]*last seen (?!never)/);
    page.once('dialog', (d) => d.accept());
    await page.click('#clients [data-action="revoke"]');
    await page.waitForFunction(() => /revoked/.test(document.querySelector('#clients .card').textContent));
    assert.equal(await page.locator('#clients [data-action="revoke"]').count(), 0);
    assert.equal(await whoami(), 401);
  });
  await step('users: Pause replies is the emergency stop for one person, shown as text, and Resume undoes it', async () => {
    await openPage('clients');
    await page.waitForSelector('#users [data-user]');
    const row = '#users [data-user]';
    assert.equal(await page.innerText(row + ' .what'), 'Ann');
    assert.equal(await page.innerText(row + ' button'), 'Pause replies');
    await page.click(row + ' button');
    await page.waitForFunction(() => /replies paused/.test(document.querySelector('#users [data-user] .what').textContent));
    assert.equal(await page.innerText(row + ' button'), 'Resume replies');
    // It is on the server's side too: the admin API says so, and a reload shows it.
    await openPage('providers');
    await openPage('clients');
    await page.waitForFunction(() => /replies paused/.test(document.querySelector('#users [data-user] .what').textContent));
    await page.click(row + ' button');
    await page.waitForFunction(() => document.querySelector('#users [data-user] .what').textContent === 'Ann');
    assert.equal(await page.innerText(row + ' button'), 'Pause replies');
    assert.equal(await page.locator('#users b').count(), 0, 'names are text');
  });
  await step('users: deleting a person needs their name typed, then removes them and nothing else', async () => {
    await openPage('clients');
    await page.waitForSelector('#users [data-user]');
    await page.fill('#u-name', 'Zed');
    await page.click('#user-form button[type=submit]');
    await page.waitForFunction(() => /Zed/.test(document.getElementById('users').textContent));
    const zed = '#users [data-user]:has(.what:text-is("Zed"))';
    await page.click(zed + ' [data-action="delete"]');
    const confirm = zed + ' [data-confirm]';
    await page.waitForSelector(confirm);
    assert.match(await page.innerText(confirm), /cannot be undone/);
    const go = confirm + ' button.danger';
    assert.equal(await page.isDisabled(go), true, 'nothing to confirm yet');
    await page.fill(confirm + ' input', 'zed');
    assert.equal(await page.isDisabled(go), true, 'the name must match exactly');
    // Cancel closes it and deletes nothing.
    await page.click(confirm + ' button.secondary');
    assert.equal(await page.locator(confirm).count(), 0);
    assert.equal(await page.locator(zed).count(), 1);
    await page.click(zed + ' [data-action="delete"]');
    await page.fill(confirm + ' input', 'Zed');
    assert.equal(await page.isDisabled(go), false);
    await page.click(go);
    await page.waitForFunction(() => !/Zed/.test(document.getElementById('users').textContent));
    assert.match(await page.innerText('#users'), /Ann/, 'the other person stays');
    const left = await page.evaluate(async () => (await (await fetch('/api/admin/v1/users')).json()).map((u) => u.name));
    assert.ok(left.includes('Ann') && !left.includes('Zed'), JSON.stringify(left));
  });
  await step('settings page saves the system prompt and display defaults', async () => {
    await openPage('settings');
    await page.waitForSelector('#settings-form');
    await page.waitForFunction(() => /quick-question popup/.test(document.getElementById('s-prompt').value));
    await page.fill('#s-prompt', 'You are a pirate.');
    await page.selectOption('#s-theme', 'dark');
    await page.fill('#s-width', '900');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#flash:not([hidden])');
    assert.match(await page.innerText('#flash'), /Settings saved/);
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
    await page.waitForSelector('#flash:not([hidden])');
    assert.match(await page.innerText('#flash'), /Settings saved/);
    await openSettings();
    await page.waitForFunction(() => /everything/.test(document.getElementById('s-notice-text').value));
    assert.equal(await page.inputValue('#s-notice-text'), 'We keep <b>everything</b> for the number of days below.', 'text, never markup');
    assert.equal(await page.isChecked('#s-warning-on'), false);
    assert.equal(await page.isChecked('#s-notice-on'), true);
    // Put it back: the default words, and both on.
    await page.fill('#s-notice-text', '');
    await page.check('#s-warning-on');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#flash:not([hidden])');
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
    assert.equal(await page.innerText('#s-gen-timeout-min'), '300 seconds = 5 minutes');
    await page.fill('#s-width', '900'); // the previous step left an invalid width in the form
    await page.fill('#s-gen-timeout', '120');
    assert.equal(await page.innerText('#s-gen-timeout-min'), '120 seconds = 2 minutes');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#flash:not([hidden])');
    assert.match(await page.innerText('#flash'), /Settings saved/);
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
    await page.waitForSelector('#flash:not([hidden])');
    await page.fill('#s-history-turns', '9'); // unsaved: only a reload from the server brings back 4
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-history-turns').value === '4');
  });
  await step('newest first: off by default, saves across a reload', async () => {
    await openSettings();
    assert.equal(await page.isChecked('#s-newest-first'), false);
    await page.check('#s-newest-first');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#flash:not([hidden])');
    await page.uncheck('#s-newest-first'); // unsaved: only a reload from the server brings it back
    await openSettings();
    assert.equal(await page.isChecked('#s-newest-first'), true);
  });
  await step('kill switch: off by default, aborts running replies by default; both save and reload', async () => {
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-gen-timeout').value === '120');
    assert.equal(await page.isChecked('#s-kill-switch'), false);
    assert.equal(await page.isChecked('#s-kill-aborts'), true);
    await page.check('#s-kill-switch');
    await page.uncheck('#s-kill-aborts');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#flash:not([hidden])');
    await page.uncheck('#s-kill-switch'); // unsaved changes, undone by the reload below
    await page.check('#s-kill-aborts');
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-kill-switch').checked === true);
    assert.equal(await page.isChecked('#s-kill-switch'), true);
    assert.equal(await page.isChecked('#s-kill-aborts'), false);
    // And back, so later steps see an ordinary server.
    await page.uncheck('#s-kill-switch');
    await page.check('#s-kill-aborts');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#flash:not([hidden])');
  });
  await step('alerts: not set up shows a banner; ntfy settings are saved, tested and removed, and never shown again (ADR 0055)', async () => {
    const TOPIC = 'e2e-topic-' + 'q7x9k2';
    const TOKEN = 'e2e-token-' + 'm4p8z1';
    const sink = (p, method = 'GET') => new Promise((resolve, reject) => {
      const req = http.request({ host: '127.0.0.1', port: ntfyPort, path: p, method }, (res) => { let b = ''; res.on('data', (c) => (b += c)); res.on('end', () => resolve(b)); });
      req.on('error', reject); req.end();
    });
    await openSettings();
    await page.waitForSelector('#alerts-form');
    await page.waitForFunction(() => !document.getElementById('alerts-banner').hidden);
    assert.match(await page.innerText('#alerts-banner'), /Alerts are not set up/);
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
    await page.waitForFunction(() => document.getElementById('alerts-banner').hidden);
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
    await page.waitForFunction(() => !document.getElementById('alerts-banner').hidden);
    assert.match(await page.innerText('#alerts-banner'), /Alerts are not set up/);
  });
  await step('alerts: email is saved, tested, kept when ntfy changes, and the password is never shown again (ADR 0055)', async () => {
    const PASSWORD = 'e2e-mail-pass-' + 'v5c2n8';
    const USER = 'e2e-mail-user-' + 'h3t6w0';
    const mail = (p, method = 'GET') => new Promise((resolve, reject) => {
      const req = http.request({ host: '127.0.0.1', port: smtpSeenPort, path: p, method }, (res) => { let b = ''; res.on('data', (c) => (b += c)); res.on('end', () => resolve(b)); });
      req.on('error', reject); req.end();
    });
    await openSettings();
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
    await page.waitForSelector('#flash:not([hidden])');
    assert.match(await page.innerText('#flash'), /Network settings saved/);
    assert.match(await page.innerText('#n-info'), /admin site is at https:\/\/askbar\.orraitu\.com\/admin/);
    assert.match(await page.innerText('#n-info'), /pairing keys point at https:\/\/askbar\.orraitu\.com\./);
    assert.equal(await page.inputValue('#n-token'), '');
    assert.match(await page.getAttribute('#n-token', 'placeholder'), /unchanged/);
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
    assert.equal(await page.inputValue('#n-challenge'), 'dns', 'the DNS challenge is the default');
    assert.equal(await page.isVisible('#n-dns-box'), true);
    assert.equal(await page.isHidden('#n-http-note'), true);
    await page.selectOption('#n-challenge', 'http');
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
    await page.waitForSelector('#flash:not([hidden])');
    assert.match(await page.innerText('#n-info'), /admin site is at https:\/\/askbar\.orraitu\.com\/admin/);
    await page.reload();
    await openPage('network');
    await page.waitForFunction(() => document.getElementById('n-host').value === 'askbar.orraitu.com');
    assert.equal(await page.inputValue('#n-challenge'), 'http');
    assert.equal(await page.isHidden('#n-dns-box'), true);
    // Back to DNS: the token box is back, and the token stored earlier is still held.
    await page.selectOption('#n-challenge', 'dns');
    assert.equal(await page.isVisible('#n-dns-box'), true);
    assert.match(await page.getAttribute('#n-token', 'placeholder'), /unchanged/);
    // Leave things as the next steps expect: plain HTTP, the DNS challenge.
    await page.uncheck('#n-tls');
    await page.click('#network-form button[type=submit]');
    await page.waitForFunction(() => /point at http:\/\/localhost/.test(document.getElementById('n-info').textContent));
    assert.equal(await page.inputValue('#n-challenge'), 'dns');
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
    await openPage('restore');
    assert.equal(await page.isHidden('#restore-form'), true, 'the passphrase is not asked for before the backup is checked');
    await page.setInputFiles('#restore-file', path.join(data, 'downloaded.tar'));
    await page.click('#restore-upload-form button[type=submit]');
    await page.waitForSelector('#restore-error:not([hidden])');
    assert.match(await page.innerText('#restore-error'), /only be restored into an empty install/);
    assert.equal(await page.isHidden('#restore-form'), true);
    // The Backup page points here, and the first-run screen has the same link (the audit step shows it).
    await openPage('backup');
    assert.equal(await page.locator('#tab-backup a[href="/admin/restore/"]').count(), 1);
  });
  await step('backup: the destination, "Back up now", the limits and the drill reminder work and show only text (ADR 0013, D27 to D29)', async () => {
    await openPage('backup');
    await page.waitForFunction(() => document.getElementById('sched-path').textContent.endsWith('/backup'));
    assert.match(await page.innerText('#sched-state'), /^writable$/);
    // The test directory is on the same disk as the admin's data, and the page says so.
    assert.match(await page.innerText('#sched-warn'), /same disk as Askbar/);
    await page.click('#sched-run');
    await page.waitForFunction(() => /^Backup made: askbar-backup-\d+\.tar$/.test(document.getElementById('flash').textContent));
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
    await page.waitForFunction(() => document.getElementById('flash').textContent === 'Saved.');
    await openPage('providers');
    await openPage('backup');
    await page.waitForFunction(() => document.getElementById('sched-keep-files').value === '3');
    assert.equal(await page.inputValue('#sched-keep-days'), '14');
    // The drill reminder shows when it is due and "Drill done" clears it.
    assert.equal(await page.isHidden('#sched-reminder'), true, 'not due yet');
  });
  // ------------------------------------------------------------------ the Audit tab
  // Against the real server: a provider whose name and model are markup, one real chat through it,
  // then the Audit tab (#tab-audit-calls, #audit-nav, the Requests rows, the waterfall, the Content view).
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
    const NASTY = '#PANEL img, #PANEL iframe, #PANEL object, #PANEL embed, #PANEL svg, #PANEL style, #PANEL base, #PANEL meta, #PANEL form, #PANEL script, #PANEL a[href^="javascript"]';
    const inert = async (panel, what) => {
      const r = await page.evaluate((nasty) => ({ nasty: document.querySelectorAll(nasty).length, pwned: window.__pwned }), NASTY.replaceAll('#PANEL', panel));
      assert.deepEqual(r, { nasty: 0, pwned: undefined }, what);
      const shown = await page.innerText(panel);
      assert.ok(shown.includes('<img src=x onerror=window.__pwned=1>'), what + ': the name is visible, as text');
      assert.ok(shown.includes('<script>window.__pwned=1</script> hello'), what + ': so is the script tag');
    };
    await openPage('clients');
    await page.waitForSelector('#users [data-user="h0"]');
    await page.waitForSelector('#clients [data-client="c0"]');
    await inert('#users', 'users');
    await inert('#clients', 'clients');
    await openPage('providers');
    await page.waitForSelector('#providers .card[data-name]');
    await inert('#providers', 'providers');
    await page.unroute('**/api/admin/v1/users');
    await page.unroute('**/api/admin/v1/clients');
    await page.unroute('**/api/admin/v1/providers');
    page.off('request', onRequest);
    assert.deepEqual(external, [], 'a request left the admin site');
    await openPage('providers');
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
    assert.match(await page.innerText('#bk-error'), /do not match/);
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
  await step('audit: the tab loads, has its five views and counts a real request', async () => {
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
    await openPage('audit/calls');
    await page.waitForSelector('#tab-audit-calls:not([hidden])');
    const subs = await page.$$eval('#audit-nav button', (b) => b.map((x) => x.dataset.sub));
    assert.deepEqual(subs, ['overview', 'requests', 'trace', 'routing', 'content']);
    await page.waitForFunction(() => Number(document.querySelector('#audit-overview [data-stat="requests"]')?.textContent) >= 1);
    assert.match(await page.innerText('#audit-by-provider'), /<img src=x/, 'the provider name is shown as text in the overview too');
  });
  await step('audit: server strings are shown as text, never as HTML, in every table', async () => {
    const nothingRan = async (where) => {
      assert.equal(await page.locator('#tab-audit-calls img').count(), 0, where + ': no element was made from a name');
      assert.equal(await page.evaluate(() => window.__pwned), undefined, where + ': nothing ran');
    };
    await nothingRan('overview');
    await page.click('#audit-nav [data-sub="requests"]');
    await page.waitForSelector('#audit-requests tr[data-trace]');
    assert.match(await page.innerText('#audit-requests'), /<img src=x/, 'the provider and model names are visible as text');
    await nothingRan('requests');
    await page.click('#audit-requests tr[data-trace]');
    await page.waitForSelector('#audit-waterfall .span-bar');
    assert.match(await page.innerText('#audit-waterfall'), /<img src=x/, 'the attempt is named after the provider');
    await page.focus('#audit-waterfall .span-bar[data-span] >> nth=-1');
    assert.match(await page.innerText('#audit-span-details'), /<img src=x/, 'the details show the model as text');
    await nothingRan('trace');
    await page.click('#audit-nav [data-sub="routing"]');
    await page.waitForSelector('#audit-attempt-rows tr');
    assert.match(await page.innerText('#audit-attempts'), /<img src=x/);
    await page.waitForSelector('#audit-routing-rows tr');
    assert.match(await page.innerText('#audit-routing-table'), /<img src=x/);
    await nothingRan('routing');
    await page.click('#audit-nav [data-sub="content"]');
    await page.waitForSelector('#audit-sealed .sealed-row');
    await nothingRan('content');
  });
  await step('audit: content stays sealed until the passphrase opens a session, and Seal closes it again', async () => {
    const asked = [];
    page.on('request', (r) => { if (/audit\/payloads/.test(r.url())) asked.push(r.url()); });
    await page.click('#audit-nav [data-sub="content"]');
    assert.match(await page.innerText('#audit-content'), /Sealed/);
    assert.equal(await page.getAttribute('#audit-pass', 'type'), 'password');
    assert.equal(asked.length, 0, 'nothing is fetched until a session is open');
    // View while sealed sends you to the passphrase: nothing is fetched.
    await page.click('#audit-content .view-btn');
    await page.waitForSelector('#audit-pass-error:not([hidden])');
    assert.match(await page.innerText('#audit-pass-error'), /Sealed/);
    assert.equal(asked.length, 0);
    // A wrong passphrase opens nothing; the field is emptied.
    await page.fill('#audit-pass', 'not the passphrase at all!!');
    await page.click('#audit-unseal');
    await page.waitForFunction(() => /does not open any audit key/.test(document.getElementById('audit-pass-error').textContent));
    assert.equal(await page.inputValue('#audit-pass'), '');
    assert.equal(await page.isHidden('#unsealed-banner'), true);
    // The right one opens a session: a banner with the time left and a Seal button, and no more prompts.
    await page.fill('#audit-pass', AUDIT_PASS);
    await page.click('#audit-unseal');
    await page.waitForSelector('#audit-open:not([hidden])');
    assert.match(await page.innerText('#audit-open-state'), /Unsealed\. It seals itself in about/);
    assert.equal(await page.inputValue('#audit-pass'), '', 'the passphrase field is cleared after use');
    await page.waitForSelector('#unsealed-banner:not([hidden])', { timeout: 15000 });
    assert.match(await page.innerText('#unsealed-text'), /Sealed content is open/);
    await page.click('#audit-content .view-btn');
    await page.waitForFunction(() => /Anything else\?/.test(document.getElementById('audit-viewed')?.textContent || ''));
    assert.equal(await page.evaluate(() => JSON.stringify([...Object.entries(localStorage), ...Object.entries(sessionStorage)]).includes('passphrase')), false);
    // The unseal is an alert in the banner area.
    await page.waitForFunction(() => /Sealed audit content was unsealed/.test(document.getElementById('sealed-alerts').textContent), null, { timeout: 15000 });
    // The access history: the view, the unseal, and the wrong guess, as text.
    await page.waitForSelector('#audit-access-rows tr');
    const results = await page.$$eval('#audit-access-rows tr td:last-child', (c) => c.map((x) => x.textContent));
    assert.deepEqual(results, ['opened', 'opened', 'wrong passphrase'], 'newest first');
    // Seal: the text is gone from the page, the form is back, the banner goes.
    await page.click('#audit-seal');
    await page.waitForSelector('#audit-unseal-form:not([hidden])');
    assert.equal((await page.content()).includes('Anything else?'), false, 'the opened text is gone from the page once sealed');
    await page.waitForSelector('#unsealed-banner', { state: 'hidden', timeout: 15000 }).catch(async () => { await page.reload(); });
    assert.equal(await page.locator('#tab-audit-calls img').count(), 0);
    assert.equal(await page.evaluate(() => window.__pwned), undefined);
  });
  await step('audit overview: a canned stats payload fills the tiles, the day bars and the provider bars', async () => {
    const canned = {
      since: 0, requests: 40, by_status: { done: 36, error: 3, cancelled: 1 }, error_rate: 0.075,
      latency_ms: { p50: 1200, p95: 5400 }, ttft_ms: { p50: 300, p95: 900 },
      tokens: { in: 12345, out: 6789, cached: 2000 }, cost: 1.2345, cost_unknown_requests: 0,
      by_provider: [{ provider_id: 'p-unknown', requests: 30, tokens: { in: 9000, out: 4000, cached: 0 }, cost: 0.9 }, { provider_id: 'p2', requests: 10, tokens: { in: 3345, out: 2789, cached: 2000 }, cost: null }],
      by_day: [{ day: '2026-11-01', requests: 10, tokens: { in: 100, out: 50, cached: 0 }, cost: 0.1 }, { day: '2026-11-02', requests: 30, tokens: { in: 12245, out: 6739, cached: 2000 }, cost: 1.1345 }],
    };
    const asked = [];
    await page.route('**/api/admin/v1/audit/stats*', (route) => { asked.push(route.request().url()); route.fulfill({ contentType: 'application/json', body: JSON.stringify(canned) }); });
    await openPage('providers');
    await openPage('audit/calls');
    await page.click('#audit-nav [data-sub="overview"]'); // the tab remembers the view it was last on
    await page.waitForSelector('#audit-overview [data-stat="requests"]');
    const stat = (k) => page.innerText(`#audit-overview [data-stat="${k}"]`);
    assert.deepEqual(
      [await stat('requests'), await stat('error-rate'), await stat('latency-p50'), await stat('latency-p95'), await stat('ttft-p50'), await stat('ttft-p95')],
      ['40', '7.5%', '1.2 s', '5.4 s', '300 ms', '900 ms'],
    );
    assert.deepEqual([await stat('tokens-in'), await stat('tokens-out'), await stat('tokens-cached'), await stat('cost')], ['12,345', '6,789', '2,000', '1.2345']);
    const days = await page.$$eval('#audit-days .bar-row', (rows) => rows.map((r) => [r.querySelector('.name').textContent, r.querySelector('.fill').style.width, r.querySelector('.figs').textContent]));
    assert.deepEqual(days.map((d) => d[0]), ['2026-11-01', '2026-11-02']);
    assert.deepEqual(days.map((d) => d[1]), ['33%', '100%'], 'bars are sized by requests, the busiest day full width');
    assert.match(days[1][2], /30 req · 18,984 tok · 1.1345/);
    const provs = await page.$$eval('#audit-by-provider .bar-row .figs', (n) => n.map((x) => x.textContent));
    assert.match(provs[1], /unknown/, 'a missing price shows as unknown, not as 0');
    // The window decides what is asked for.
    const since = (u) => Number(new URL(u).searchParams.get('since'));
    const now = Math.floor(Date.now() / 1000);
    assert.ok(Math.abs(now - since(asked.at(-1)) - 86400) < 120, 'the first look is the last 24 hours');
    await page.selectOption('#audit-window', '604800');
    await page.waitForTimeout(200);
    assert.ok(Math.abs(now - since(asked.at(-1)) - 7 * 86400) < 120, 'then the last 7 days');
    await page.selectOption('#audit-window', '2592000');
    await page.waitForTimeout(200);
    assert.ok(Math.abs(now - since(asked.at(-1)) - 30 * 86400) < 120, 'then the last 30 days');
    // An empty window says so instead of showing zeros.
    await page.unroute('**/api/admin/v1/audit/stats*');
    await page.route('**/api/admin/v1/audit/stats*', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ requests: 0, by_status: {}, error_rate: 0, latency_ms: { p50: null, p95: null }, tokens: { in: 0, out: 0, cached: 0 }, cost: 0, cost_unknown_requests: 0, by_provider: [], by_day: [] }) }));
    await page.selectOption('#audit-window', '86400');
    await page.waitForSelector('#audit-overview-empty:not([hidden])');
    assert.equal(await page.isHidden('#audit-tiles'), true);
    await page.unroute('**/api/admin/v1/audit/stats*');
    await page.selectOption('#audit-window', '86400');
  });
  await step('audit requests: a canned list fills the table, filters and load-more ask for the right pages', async () => {
    const row = (n, extra = {}) => ({ trace_id: 'T' + String(n).padStart(3, '0'), span_id: 's' + n, status: 'done', started_at: 1793631845 - n, ended_at: 1793631846 - n, user_id: 'user-1', client_id: 'd', tokens_in: 100, tokens_out: 50, tokens_cached: 0, error_code: null, provider_id: 'pA', model: 'model-' + n, ttft_ms: 200, latency_ms: 1500, cost: 0.0007, ...extra });
    const first = Array.from({ length: 25 }, (_, i) => row(100 - i));
    const second = Array.from({ length: 15 }, (_, i) => row(75 - i));
    const asked = [];
    await page.route('**/api/admin/v1/audit/traces?*', (route) => {
      const q = new URL(route.request().url()).searchParams;
      asked.push(Object.fromEntries(q));
      let traces = first;
      if (q.get('before') === 'T076') traces = second;
      if (q.get('status') === 'error') traces = [row(5, { status: 'error', error_code: 'http_401', tokens_in: null, tokens_out: null, cost: null })];
      if (q.get('provider') === 'pNone') traces = [];
      route.fulfill({ contentType: 'application/json', body: JSON.stringify({ traces }) });
    });
    await openPage('providers');
    await openPage('audit/calls');
    await page.click('#audit-nav [data-sub="requests"]');
    await page.waitForSelector('#audit-rows tr[data-trace]');
    assert.deepEqual(asked[asked.length - 1], { limit: '25' }, 'no filters, no cursor');
    assert.equal(await page.locator('#audit-rows tr').count(), 25);
    const heads = await page.$$eval('#audit-table th', (h) => h.map((x) => x.textContent));
    assert.deepEqual(heads, ['Time', 'User', 'Provider', 'Model', 'Status', 'Latency', 'Tokens (in → out)', 'Cost']);
    const cells = await page.$$eval('#audit-rows tr:first-child td', (c) => c.map((x) => x.textContent));
    assert.deepEqual(cells.slice(1), ['user-1', 'pA', 'model-100', 'done', '1.5 s', '100 → 50', '0.0007']);

    // Load more asks for the page after the last row shown, and appends; a short page ends it.
    await page.click('#audit-more');
    await page.waitForFunction(() => document.querySelectorAll('#audit-rows tr').length === 40);
    assert.equal(asked[asked.length - 1].before, 'T076');
    assert.equal(await page.isHidden('#audit-more'), true, 'fewer than a full page means no more');
    assert.equal(await page.locator('#audit-rows tr:last-child td:nth-child(4)').innerText(), 'model-61');

    // The status filter starts again from the top with the status in the query; a failed request has no cost.
    await page.selectOption('#audit-f-status', 'error');
    await page.waitForFunction(() => document.querySelectorAll('#audit-rows tr').length === 1);
    assert.equal(asked[asked.length - 1].status, 'error');
    assert.equal(asked[asked.length - 1].before, undefined);
    const failed = await page.$$eval('#audit-rows tr:first-child td', (c) => c.map((x) => x.textContent));
    assert.deepEqual(failed.slice(4), ['error', '1.5 s', '–', '–']);
    // A filter with no matches says so.
    await page.selectOption('#audit-f-status', '');
    await page.waitForFunction(() => document.querySelectorAll('#audit-rows tr').length === 25);
    await page.evaluate(() => { const o = document.createElement('option'); o.value = 'pNone'; o.textContent = 'ghost'; document.getElementById('audit-f-provider').appendChild(o); });
    await page.selectOption('#audit-f-provider', 'pNone');
    await page.waitForSelector('#audit-requests-empty:not([hidden])');
    assert.match(await page.innerText('#audit-requests-empty'), /match these filters/);
    assert.equal(asked[asked.length - 1].provider, 'pNone');
    await page.selectOption('#audit-f-provider', '');
    await page.waitForFunction(() => document.querySelectorAll('#audit-rows tr').length === 25);

    // A row opens its trace (by click or keyboard).
    await page.route('**/api/admin/v1/audit/traces/*', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ trace_id: route.request().url().split('/').pop(), cost: null, spans: [] }) }));
    await page.click('#audit-rows tr[data-trace="T099"]');
    assert.equal(await page.getAttribute('#audit-nav [data-sub="trace"]', 'aria-selected'), 'true');
    await page.waitForFunction(() => /T099/.test(document.getElementById('audit-trace-head').textContent));
    await page.click('#audit-nav [data-sub="requests"]');
    await page.focus('#audit-rows tr[data-trace="T098"]');
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => /T098/.test(document.getElementById('audit-trace-head').textContent));
    await page.unroute('**/api/admin/v1/audit/traces/*');
    await page.unroute('**/api/admin/v1/audit/traces?*');
  });
  await step('audit trace: the waterfall layout is right for fixed numbers, shows the tree, and copes with a running span', async () => {
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
    await page.waitForSelector('#audit-waterfall .span-bar');
    assert.equal(await page.locator('#audit-waterfall .span-bar').count(), 4);
    assert.equal(await page.locator('#audit-waterfall .span-bar.status-error').count(), 1);
    assert.match(await page.innerText('#audit-trace-head'), /TRACE1 · done · .* · cost 0\.0007/);
    // total = 1002000 - 999990 = 2010 ms; c-attempt starts 1010 ms in and runs 900 ms.
    assert.equal(await page.$eval('[data-span="c-attempt"]', (b) => b.style.width), '44.78%');
    assert.equal(await page.$eval('[data-span="c-attempt"]', (b) => b.style.left), '50.25%');
    assert.equal(await page.$eval('[data-span="b-attempt"]', (b) => b.closest('.span-row').querySelector('.label').style.paddingLeft), '14px');
    await page.focus('[data-span="b-attempt"]');
    const details = await page.innerText('#audit-span-details');
    assert.match(details, /attempt · prov · error/);
    assert.match(details, /starts at \+210 ms, takes 500 ms/);
    assert.match(details, /error http_429/);
    assert.match(details, /"retry_index":0/);
    // A running span is drawn in its own style, up to now.
    await page.evaluate((s) => window.askbarAudit.renderTrace({ trace_id: 'TRACE2', cost: null, spans: s }), spans);
    assert.equal(await page.locator('#audit-waterfall .span-bar.running').count(), 1);
    assert.equal(await page.locator('#audit-waterfall .span-bar').count(), 5);
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
    await openPage('audit/calls');
    await page.evaluate(() => window.askbarAudit.openTrace('ROUTE1'));
    await page.waitForSelector('#audit-waterfall .span-bar');
    await page.click('#audit-nav [data-sub="routing"]');
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
  await step('audit content (canned): sealed until the passphrase, then View shows text as text, and a session that ends wipes the page', async () => {
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
    await openPage('providers');
    await openPage('audit/calls');
    await page.evaluate(() => window.askbarAudit.openTrace('SEAL1'));
    await page.waitForSelector('#audit-waterfall .span-bar');
    await page.click('#audit-nav [data-sub="content"]');
    // Sealed, with a View button only for spans that have something; nothing fetched yet.
    assert.equal(await page.locator('#audit-sealed .sealed-row').count(), 2);
    assert.match(await page.innerText('#audit-sealed'), /Sealed \(1 item\)/);
    assert.match(await page.innerText('#audit-sealed'), /Sealed \(2 items\)/);
    assert.equal(viewed.length, 0, 'nothing is fetched before a session is open');
    assert.equal(await page.isVisible('#audit-unseal-form'), true);

    // View while sealed: the way to the passphrase, and still nothing fetched.
    await page.click('#audit-sealed [data-span="sp-attempt"] .view-btn');
    assert.match(await page.innerText('#audit-pass-error'), /Enter the audit passphrase/);
    assert.equal(viewed.length, 0);

    // Wrong: the server's sentence, nothing opened, the field emptied.
    await page.fill('#audit-pass', 'a wrong passphrase of length');
    await page.click('#audit-unseal');
    await page.waitForFunction(() => /does not open any audit key/.test(document.getElementById('audit-pass-error').textContent));
    assert.equal(await page.inputValue('#audit-pass'), '', 'the passphrase field is cleared after use');
    assert.equal(await page.isVisible('#audit-open'), false);

    // Right: the session opens, with the keys it holds; no prompt for each item from here.
    await page.fill('#audit-pass', 'the right passphrase here');
    await page.click('#audit-unseal');
    await page.waitForSelector('#audit-open:not([hidden])');
    assert.match(await page.innerText('#audit-open-state'), /Keys open: k1\./);
    assert.equal(await page.isVisible('#audit-unseal-form'), false);
    await page.click('#audit-sealed [data-span="sp-attempt"] .view-btn');
    await page.waitForFunction(() => /that was asked/.test(document.getElementById('audit-viewed').textContent));
    const plain = await page.innerText('#audit-viewed');
    assert.match(plain, /request \(key k1\)/);
    assert.match(plain, /the <b>question<\/b> that was asked/, 'markup is shown as typed');
    assert.equal(await page.locator('#audit-viewed b').count(), 0);
    assert.match(plain, /not opened: this session has no key for it; key k0 was made .* and retired/);
    assert.ok(viewed.every((u) => /sp-attempt$/.test(u)));
    assert.deepEqual(sent, ['a wrong passphrase of length', 'the right passphrase here']);

    // The session ends on its own (a timeout, sign-out elsewhere): the page is wiped and asks again.
    open = false;
    await page.evaluate(() => window.askbarAudit.sessionFromStatus({ open: false }));
    assert.equal(await page.isVisible('#audit-unseal-form'), true);
    assert.equal((await page.content()).includes('that was asked'), false, 'wiped when the session ended');
    // A view that finds it sealed says so and shows the passphrase field again.
    await page.evaluate(() => { window.askbarAudit.session.open = true; });
    await page.click('#audit-sealed [data-span="sp-attempt"] .view-btn');
    await page.waitForFunction(() => /sealed/i.test(document.getElementById('audit-pass-error').textContent));
    // Seal from the page.
    await page.fill('#audit-pass', 'the right passphrase here');
    await page.click('#audit-unseal');
    await page.waitForSelector('#audit-open:not([hidden])');
    await page.click('#audit-seal');
    await page.waitForSelector('#audit-unseal-form:not([hidden])');
    assert.equal(open, false, 'Seal reached the server');
    assert.equal(await page.evaluate(() => JSON.stringify([...Object.entries(localStorage), ...Object.entries(sessionStorage)]).includes('passphrase')), false, 'never stored');
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
    await openPage('providers');
    await openPage('audit/calls');
    await page.click('#audit-nav [data-sub="content"]');
    await page.waitForSelector('#audit-access-rows tr');
    const cells = await page.$$eval('#audit-access-rows tr', (r) => r.map((tr) => [...tr.children].map((td) => td.textContent).slice(1)));
    assert.deepEqual(cells, [['span-<b>x</b>', '0011223344556677', 'wrong passphrase'], ['span-1', '0011223344556677', 'opened']]);
    assert.equal(await page.locator('#audit-access b').count(), 0, 'shown as text');
    assert.equal(await page.isHidden('#audit-access-empty'), true);
    rows = [];
    await page.click('#audit-nav [data-sub="requests"]');
    await page.click('#audit-nav [data-sub="content"]');
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
    await page.waitForSelector('#flash:not([hidden])');
    await page.fill('#s-retention', '999'); // unsaved, so the reload shows what was stored
    await openSettings();
    await page.waitForFunction(() => document.getElementById('s-retention').value === '45');

    // Rotation: each refusal in turn, then success.
    await page.evaluate(() => document.getElementById('rotate-form').noValidate = true);
    const NEW_PASS = 'a different long passphrase 77';
    const rotate = async (cur, next, next2) => {
      await page.fill('#r-current', cur);
      await page.fill('#r-new', next);
      await page.fill('#r-new2', next2);
      await page.click('#rotate-form button[type=submit]');
    };
    await rotate(AUDIT_PASS, NEW_PASS, NEW_PASS + 'x');
    assert.match(await page.innerText('#rotate-error'), /do not match/);
    await rotate(AUDIT_PASS, 'too short', 'too short');
    assert.match(await page.innerText('#rotate-error'), /at least 20/);
    await rotate('not the current passphrase!!', NEW_PASS, NEW_PASS);
    await page.waitForFunction(() => /current passphrase is wrong/.test(document.getElementById('rotate-error').textContent));
    assert.equal(await page.inputValue('#r-current'), '', 'the current passphrase is cleared after a try');
    assert.equal(await page.innerText('#s-key-id'), oldKey, 'nothing changed');
    await rotate(AUDIT_PASS, NEW_PASS, NEW_PASS);
    await page.waitForSelector('#flash:not([hidden])');
    assert.match(await page.innerText('#flash'), /audit key was rotated/);
    const newKey = await page.innerText('#s-key-id');
    assert.match(newKey, /^[0-9a-f]{16}$/);
    assert.notEqual(newKey, oldKey);
    for (const id of ['r-current', 'r-new', 'r-new2']) assert.equal(await page.inputValue('#' + id), '', id);
    assert.equal((await page.content()).includes(NEW_PASS), false);
    // Put the retention back so later steps see an ordinary server.
    await page.fill('#s-retention', '30');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#flash:not([hidden])');
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
    await page.waitForSelector('#flash:not([hidden])');
    assert.match(await page.innerText('#flash'), /Password changed/);
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
    assert.deepEqual(slugs, ['audit/calls', 'audit/admin', 'audit/sealed', 'audit/routing', 'providers', 'clients', 'services', 'network', 'security', 'backup', 'restore', 'settings', 'reference']);
    for (const slug of slugs) {
      await page.goto(`http://127.0.0.1:${adminPort}/admin/${slug}/`);
      await page.waitForSelector('#app:not([hidden])');
      assert.equal(await page.getAttribute(`#nav a[data-tab="${slug}"]`, 'aria-current'), 'page', slug);
      assert.equal(await page.locator('#nav a[aria-current="page"]').count(), 1, slug + ': one current page');
      assert.equal(await page.locator(`section[data-tab="${slug}"]`).count(), 1, slug + ': its own section');
      assert.equal(await page.locator('section[data-tab]').count(), 1, slug + ': and no other');
      for (const id of ['#logout', '#offline', '#flash', '#login', '#setup']) assert.equal(await page.locator(id).count(), 1, slug + ' ' + id);
      assert.match(await page.title(), / · Askbar admin$/);
    }
    await openPage('providers');
    await openPage('clients');
    await page.goBack();
    await page.waitForURL('**/admin/providers/');
    await page.waitForSelector('#app:not([hidden])');
    assert.equal(await page.getAttribute('#nav a[data-tab="providers"]', 'aria-current'), 'page', 'Back returns to the page before');
    await page.goto(`http://127.0.0.1:${adminPort}/admin`);
    assert.match(page.url(), /\/admin\/providers\/$/, '/admin goes to the first page');
    const missing = await page.goto(`http://127.0.0.1:${adminPort}/admin/nothing-here/`);
    assert.equal(missing.status(), 404, 'an unknown admin page is a 404');
    await page.goto(base);
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
    const mainLeft = () => page.evaluate(() => document.querySelector('#app > main').getBoundingClientRect().left);
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
        inert: document.querySelector('#app > main').inert,
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
    assert.equal(await page.evaluate(() => document.querySelector('#app > main').inert), false);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), true, 'no sideways scroll');
    // A page from the menu.
    await openPage('security');
    assert.equal(await page.isHidden('#side-menu'), true, 'the new page opens with the menu closed');
    await page.goto(base);
    await page.waitForSelector('#app:not([hidden])');
  });
  await step('pages: an audit request opens directly from its address, and the address follows the view', async () => {
    const list = await page.evaluate(async () => (await fetch('/api/admin/v1/audit/traces', { credentials: 'same-origin' })).json());
    const id = list.traces[0].trace_id;
    await page.goto(`http://127.0.0.1:${adminPort}/admin/audit/?view=trace&id=${encodeURIComponent(id)}`);
    await page.waitForSelector('#audit-trace:not([hidden])');
    assert.equal(await page.getAttribute('#audit-nav [data-sub="trace"]', 'aria-selected'), 'true');
    await page.waitForFunction((t) => window.askbarAudit.state.traceId === t && !!window.askbarAudit.state.trace, id);
    assert.match(page.url(), new RegExp('[?]view=trace&id=' + encodeURIComponent(id) + '$'), 'the address keeps the request');
    await page.click('#audit-nav [data-sub="requests"]');
    assert.match(page.url(), /\/admin\/audit\/calls\/\?view=requests$/, 'the old address opened Model calls');
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
    await page.waitForSelector('#login-error:not([hidden])');
    await page.fill('#password', 'brand new password 1');
    await page.click('#login-form button');
    await page.waitForSelector('#app:not([hidden])');
  });
  await step('with the server stopped the admin still loads and signs in, and says the server is offline', async () => {
    serverProc.kill();
    await new Promise((r) => serverProc.once('exit', r));
    await page.reload();
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#offline:not([hidden])', { timeout: 15000 });
    assert.match(await page.innerText('#offline'), /Server offline/);
    // Sign out and in again with the server away: the admin has its own login.
    await openMenu();
    await page.click('#logout');
    await page.waitForSelector('#login:not([hidden])');
    await page.fill('#password', 'brand new password 1');
    await page.click('#login-form button');
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForSelector('#offline:not([hidden])');
  });
  await step('audit: with the server away the trail still reads from the auditor, and the views are there', async () => {
    await openPage('audit/calls');
    await page.waitForSelector('#audit-overview');
    assert.equal(await page.isHidden('#audit-error'), true, 'the auditor is up, so there is nothing to explain');
    for (const sub of ['overview', 'requests', 'trace', 'routing', 'content']) {
      await page.click(`#audit-nav [data-sub="${sub}"]`);
      assert.equal(await page.isVisible(`#audit-${sub}`), true, sub);
      assert.equal(await page.getAttribute(`#audit-nav [data-sub="${sub}"]`, 'aria-selected'), 'true');
      assert.equal(await page.locator('#audit-nav [aria-selected="true"]').count(), 1);
      assert.match(await page.innerText(`#audit-${sub}`), /\S/, `${sub} says something even when empty`);
    }
    await page.click('#audit-nav [data-sub="overview"]');
  });
  await step('a change while the server is away is refused with a clear message and saves nothing', async () => {
    await openPage('providers');
    await page.click('#add-provider');
    await page.fill('#p-name', 'while-away');
    await page.fill('#p-model', 'm');
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('#provider-error:not([hidden])');
    assert.match(await page.innerText('#provider-error'), /offline/i);
    await page.click('#provider-cancel');
    assert.equal(await page.locator('.card[data-name="while-away"]').count(), 0);
    // What the admin owns can still be read.
    assert.equal(await page.locator('.card[data-name="mock"]').count(), 1);
  });
  await step('when the server is back the banner goes away and changes work again', async () => {
    serverProc = startServer();
    await waitForPort(api);
    await page.waitForSelector('#offline', { state: 'hidden', timeout: 30000 });
    await page.click('#add-provider');
    await page.fill('#p-name', 'after-return');
    await page.fill('#p-model', 'm');
    await page.click('#provider-form button[type=submit]');
    await page.waitForSelector('.card[data-name="after-return"]');
  });
  await step('a token the server stops accepting gets its own banner with the command, and it clears when the server accepts it again', async () => {
    const good = fs.readFileSync(hashFile, 'utf8');
    // The server's side is rotated: its hash file now holds the hash of some other token (read per request).
    fs.chmodSync(path.dirname(hashFile), 0o700);
    fs.rmSync(hashFile);
    fs.writeFileSync(hashFile, 'f'.repeat(64) + '\n');
    await page.waitForSelector('#rejected:not([hidden])', { timeout: 20000 });
    const banner = await page.innerText('#rejected');
    assert.match(banner, /rejected this admin's token/);
    assert.match(banner, /bootstrap rotate admin-server/);
    assert.match(banner, /docker compose restart server admin/);
    assert.equal(await page.isVisible('#offline'), false, 'one message, the useful one');
    fs.writeFileSync(hashFile, good);
    await page.waitForSelector('#rejected', { state: 'hidden', timeout: 20000 });
  });
  await step('audit chain: a rollback, a rewrite and an edit each get a banner with its sentence, and it clears', async () => {
    const real = await page.evaluate(async () => (await fetch('/api/admin/v1/status')).json());
    assert.equal(real.audit_chain.state === 'ok' || real.audit_chain.state === 'unknown', true, 'nothing is wrong with the real chain');
    const withChain = async (audit_chain) => {
      await page.unroute('**/api/admin/v1/status');
      await page.route('**/api/admin/v1/status', (route) => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ ...real, audit_chain }) }));
      await page.reload();
      await page.waitForSelector('#app:not([hidden])');
    };
    await withChain({ state: 'rollback', detail: 'The audit trail is shorter than it was: <b>x</b>' });
    await page.waitForSelector('#chain-alert:not([hidden])');
    assert.equal(await page.innerText('#chain-alert'), 'The audit trail is shorter than it was: <b>x</b>', 'the sentence is shown as text');
    assert.equal(await page.locator('#chain-alert b').count(), 0);
    await withChain({ state: 'rewritten', detail: 'The audit trail was rewritten.' });
    await page.waitForSelector('#chain-alert:not([hidden])');
    assert.match(await page.innerText('#chain-alert'), /rewritten/);
    await withChain({ state: 'edited', detail: 'Records in the audit trail do not match their entries.' });
    await page.waitForSelector('#chain-alert:not([hidden])');
    await withChain({ state: 'ok', detail: '' });
    await page.waitForSelector('#chain-alert', { state: 'hidden' });
    await page.unroute('**/api/admin/v1/status');
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
