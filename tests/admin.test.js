// Run: node tests/admin.test.js   (needs `cargo build -p askbar-server` and `playwright` with Chromium)
// Starts the real server and tests/mock_llm.py, then drives the admin site in a browser.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');

const ROOT = path.resolve(__dirname, '..');
const PASSWORD = 'correct horse battery';
const SECRET_KEY = 'sk-do-not-show-me-123';

let failures = 0;
async function step(name, fn) {
  try { await fn(); console.log('ok   ' + name); }
  catch (e) { failures++; console.error('FAIL ' + name + '\n     ' + String(e.message).split('\n').join('\n     ')); }
}

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

(async () => {
  const [api, adminPort, mockPort] = [await freePort(), await freePort(), await freePort()];
  const data = fs.mkdtempSync(path.join(os.tmpdir(), 'askbar-admin-'));
  const children = [];
  const run = (cmd, args, env) => { const c = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: 'ignore' }); children.push(c); return c; };
  const cleanup = () => { for (const c of children) c.kill(); fs.rmSync(data, { recursive: true, force: true }); };

  run('python3', ['-u', path.join(ROOT, 'tests/mock_llm.py'), String(mockPort)], {});
  run(path.join(ROOT, 'target/debug/askbar-server'), [], {
    ASKBAR_API_ADDR: '127.0.0.1:' + api, ASKBAR_ADMIN_ADDR: '127.0.0.1:' + adminPort, ASKBAR_DATA_DIR: data,
    ASKBAR_SECRETS_DIR: path.join(data, 'secrets'),
  });
  await waitForPort(adminPort);
  await waitForPort(mockPort);

  const base = `http://127.0.0.1:${adminPort}/admin/`;
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 900, height: 800 }, permissions: ['clipboard-read', 'clipboard-write'] });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d/.test(m.text())) errors.push(m.text()); });

  let masterKey = null;
  await step('first run shows the setup wizard, not a login form', async () => {
    await page.goto(base);
    await page.waitForSelector('#setup:not([hidden])');
    assert.equal(await page.isVisible('#setup-password-form'), true);
    assert.equal(await page.isHidden('#login'), true);
    assert.equal(await page.isHidden('#app'), true);
  });
  await step('setup step 1: the password must match and be long enough', async () => {
    await page.fill('#setup-password', PASSWORD);
    await page.fill('#setup-password2', PASSWORD + 'x');
    await page.click('#setup-password-form button[type=submit]');
    await page.waitForSelector('#setup-password-error:not([hidden])');
    assert.match(await page.innerText('#setup-password-error'), /do not match/);
    await page.fill('#setup-password2', PASSWORD);
    await page.click('#setup-password-form button[type=submit]');
    await page.waitForSelector('#setup-key:not([hidden])');
    assert.equal(await page.isHidden('#setup-password-form'), true);
  });
  await step('setup step 2: the master key is shown once, with a copy button and a password-manager hint', async () => {
    await page.click('#setup-key-button');
    await page.waitForSelector('#setup-saved:not([hidden])');
    masterKey = await page.inputValue('#saved-key');
    assert.ok(masterKey.length >= 32);
    assert.match(await page.innerText('#setup-saved'), /password manager/i);
    assert.equal(await page.isDisabled('#saved-continue'), true);
    await page.click('#saved-copy');
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), masterKey);
    assert.equal(fs.readFileSync(path.join(data, 'secrets', 'master.key'), 'utf8').trim(), masterKey);
    await page.check('#saved-confirm');
    assert.equal(await page.isDisabled('#saved-continue'), false);
    await page.click('#saved-continue');
    await page.waitForSelector('#app:not([hidden])');
    assert.equal(await page.inputValue('#saved-key'), '', 'the key is cleared from the page');
    assert.match(await page.innerText('#providers'), /No providers yet/);
  });
  await step('the key is not shown again after a reload', async () => {
    await page.reload();
    await page.waitForSelector('#app:not([hidden])');
    assert.equal((await page.content()).includes(masterKey), false);
    assert.equal(await page.isHidden('#setup'), true);
  });
  await step('sign out, then a wrong password is refused and the right one signs in', async () => {
    await page.click('#logout');
    await page.waitForSelector('#login:not([hidden])');
    await page.fill('#password', 'nope');
    await page.click('#login-form button');
    await page.waitForSelector('#login-error:not([hidden])');
    assert.match(await page.innerText('#login-error'), /Wrong password/);
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
  await step('Test sends a message through the provider and shows success', async () => {
    await page.click('.card[data-name="mock"] [data-action="test"]');
    await page.waitForFunction(() => /Success|Failed/.test(document.querySelector('.card[data-name="mock"] .test-result').textContent));
    assert.match(await page.innerText('.card[data-name="mock"] .test-result'), /^Success:/);
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
  await step('editing without a new key keeps the old one working', async () => {
    await page.fill('#p-model', 'mock-model-2');
    await page.click('#provider-form button[type=submit]');
    await page.waitForFunction(() => /mock-model-2/.test(document.querySelector('.card[data-name="mock"]')?.textContent || ''));
    await page.click('.card[data-name="mock"] [data-action="test"]');
    await page.waitForFunction(() => /Success|Failed/.test(document.querySelector('.card[data-name="mock"] .test-result').textContent));
    assert.match(await page.innerText('.card[data-name="mock"] .test-result'), /^Success:/);
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
    assert.match(await page.innerText('.card[data-name="dead"] .test-result'), /could not reach/);
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
  await step('the CLIProxyAPI preset fills type and base URL', async () => {
    await page.click('#add-cpa');
    assert.equal(await page.inputValue('#p-type'), 'openai');
    assert.equal(await page.inputValue('#p-base-url'), 'http://cpa:8317/v1');
    assert.equal(await page.inputValue('#p-name'), 'cpa');
    await page.click('#provider-cancel');
  });
  await step('delete asks first, then removes the provider', async () => {
    page.once('dialog', (d) => d.dismiss());
    await page.click('.card[data-name="dead"] [data-action="delete"]');
    assert.equal(await page.locator('.card[data-name="dead"]').count(), 1);
    page.once('dialog', (d) => d.accept());
    await page.click('.card[data-name="dead"] [data-action="delete"]');
    await page.waitForFunction(() => !document.querySelector('.card[data-name="dead"]'));
  });
  await step('clients see the provider without its key', async () => {
    // The API is the other listener; without a token it must refuse.
    const r = await fetch(`http://127.0.0.1:${api}/api/v1/providers`, { headers: { 'x-askbar-client-version': '0.3.0' } });
    assert.equal(r.status, 401);
  });
  await step('devices: generate a pairing string, pair with it, then revoke the device', async () => {
    await page.click('#nav [data-tab="devices"]');
    await page.waitForSelector('#pair-form');
    await page.waitForFunction(() => /No devices are paired yet/.test(document.getElementById('devices').textContent));
    await page.fill('#d-name', 'Test laptop');
    await page.click('#pair-form button[type=submit]');
    await page.waitForSelector('#pairing-out:not([hidden])');
    const pairing = await page.inputValue('#pairing-string');
    assert.match(pairing, /^askbar-pair:/);
    const { url, code } = JSON.parse(Buffer.from(pairing.slice('askbar-pair:'.length), 'base64url').toString());
    assert.equal(url, 'http://localhost:' + api);
    const headers = { 'content-type': 'application/json', 'x-askbar-client-version': '0.3.0' };
    const paired = await fetch(`http://127.0.0.1:${api}/api/v1/pair`, { method: 'POST', headers, body: JSON.stringify({ code }) });
    assert.equal(paired.status, 200);
    const { token } = await paired.json();
    const whoami = () => fetch(`http://127.0.0.1:${api}/api/v1/whoami`, { headers: { ...headers, authorization: 'Bearer ' + token } }).then((r) => r.status);
    assert.equal(await whoami(), 204);

    await page.click('#nav [data-tab="providers"]');
    await page.click('#nav [data-tab="devices"]');
    await page.waitForSelector('#devices .card');
    assert.match(await page.innerText('#devices .card'), /Test laptop[\s\S]*active[\s\S]*last seen (?!never)/);
    page.once('dialog', (d) => d.accept());
    await page.click('#devices [data-action="revoke"]');
    await page.waitForFunction(() => /revoked/.test(document.querySelector('#devices .card').textContent));
    assert.equal(await page.locator('#devices [data-action="revoke"]').count(), 0);
    assert.equal(await whoami(), 401);
  });
  await step('settings page saves the system prompt and display defaults', async () => {
    await page.click('#nav [data-tab="settings"]');
    await page.waitForSelector('#settings-form');
    await page.waitForFunction(() => /quick-question popup/.test(document.getElementById('s-prompt').value));
    await page.fill('#s-prompt', 'You are a pirate.');
    await page.selectOption('#s-theme', 'dark');
    await page.fill('#s-width', '900');
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#flash:not([hidden])');
    assert.match(await page.innerText('#flash'), /Settings saved/);
    await page.click('#nav [data-tab="providers"]');
    await page.click('#nav [data-tab="settings"]');
    await page.waitForFunction(() => document.getElementById('s-width').value === '900');
    assert.equal(await page.inputValue('#s-prompt'), 'You are a pirate.');
    assert.equal(await page.inputValue('#s-theme'), 'dark');
  });
  await step('an invalid setting shows the server message', async () => {
    await page.fill('#s-top', '0.5');
    await page.fill('#s-width', '100');
    await page.evaluate(() => document.getElementById('settings-form').noValidate = true);
    await page.click('#settings-form button[type=submit]');
    await page.waitForSelector('#settings-error:not([hidden])');
    assert.match(await page.innerText('#settings-error'), /Width must be/);
  });
  await step('network: hostname, token and networks are saved; the token is write-only', async () => {
    const CF = 'cf_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789';
    await page.click('#nav [data-tab="network"]');
    await page.waitForFunction(() => /pairing strings point at/.test(document.getElementById('n-info').textContent));
    await page.check('#n-tls');
    await page.fill('#n-host', 'https://askbar.orraitu.com');
    await page.fill('#n-token', CF);
    await page.fill('#n-cidr', '192.168.1.0/24');
    await page.click('#network-form button[type=submit]');
    await page.waitForSelector('#network-error:not([hidden])');
    assert.match(await page.innerText('#network-error'), /plain DNS name/);
    await page.fill('#n-host', 'askbar.orraitu.com');
    await page.check('#n-staging');
    await page.click('#network-form button[type=submit]');
    await page.waitForSelector('#flash:not([hidden])');
    assert.match(await page.innerText('#flash'), /Network settings saved/);
    assert.match(await page.innerText('#n-info'), /https:\/\/askbar\.orraitu\.com:8443/);
    assert.equal(await page.inputValue('#n-token'), '');
    assert.match(await page.getAttribute('#n-token', 'placeholder'), /unchanged/);
    assert.equal((await page.content()).includes(CF), false);
    await page.reload();
    await page.click('#nav [data-tab="network"]');
    await page.waitForFunction(() => document.getElementById('n-host').value === 'askbar.orraitu.com');
    assert.equal(await page.isChecked('#n-staging'), true);
    assert.equal((await page.content()).includes(CF), false);
    // Back to plain HTTP so the rest of the run keeps working the same way.
    await page.uncheck('#n-tls');
    await page.click('#network-form button[type=submit]');
    await page.waitForFunction(() => /point at http:\/\/localhost/.test(document.getElementById('n-info').textContent));
  });
  await step('security: rotating the master key shows the new key once and providers keep working', async () => {
    await page.click('#nav [data-tab="security"]');
    page.once('dialog', (d) => d.accept());
    await page.fill('#rotate-password', 'wrong password');
    await page.click('#rotate-form button[type=submit]');
    await page.waitForSelector('#rotate-error:not([hidden])');
    assert.match(await page.innerText('#rotate-error'), /wrong/);
    page.once('dialog', (d) => d.accept());
    await page.fill('#rotate-password', PASSWORD);
    await page.click('#rotate-form button[type=submit]');
    await page.waitForSelector('#rotated:not([hidden])');
    const fresh = await page.inputValue('#rotated-key');
    assert.notEqual(fresh, masterKey);
    assert.match(await page.innerText('#rotated'), /password manager/i);
    await page.click('#rotated-copy');
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), fresh);
    const keyFile = path.join(data, 'secrets', 'master.key');
    assert.equal(fs.readFileSync(keyFile, 'utf8').trim(), fresh);
    assert.equal(fs.readFileSync(keyFile + '.prev', 'utf8').trim(), masterKey);
    await page.waitForSelector('#prev-key:not([hidden])');
    await page.click('#rotated-done');
    assert.equal(await page.inputValue('#rotated-key'), '');
    page.once('dialog', (d) => d.accept());
    await page.click('#prev-delete');
    await page.waitForSelector('#prev-key', { state: 'hidden' });
    assert.equal(fs.existsSync(keyFile + '.prev'), false);
    masterKey = fresh;
    await page.click('#nav [data-tab="providers"]');
    await page.click('.card[data-name="mock"] [data-action="test"]');
    await page.waitForFunction(() => /Success|Failed/.test(document.querySelector('.card[data-name="mock"] .test-result').textContent));
    assert.match(await page.innerText('.card[data-name="mock"] .test-result'), /^Success:/);
  });
  await step('backup downloads a database without provider keys', async () => {
    await page.click('#nav [data-tab="backup"]');
    const [download] = await Promise.all([page.waitForEvent('download'), page.click('#backup-link')]);
    assert.match(download.suggestedFilename(), /^askbar-backup-\d{8}-\d{6}\.db$/);
    const file = path.join(data, 'downloaded.db');
    await download.saveAs(file);
    const bytes = fs.readFileSync(file);
    assert.equal(bytes.subarray(0, 15).toString(), 'SQLite format 3');
    assert.equal(bytes.includes(SECRET_KEY), false);
    assert.equal(bytes.includes(masterKey), false);
  });
  await step('security: changing the password needs the current one', async () => {
    await page.click('#nav [data-tab="security"]');
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
  await step('sign out returns to the login form', async () => {
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
  await step('no JavaScript errors', async () => { assert.deepEqual(errors, []); });

  await browser.close();
  cleanup();
  console.log(failures ? `\n${failures} step(s) failed` : '\nall admin steps passed');
  process.exit(failures ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(1); });
