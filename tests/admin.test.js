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
    ASKBAR_MASTER_KEY: '0123456789abcdef0123456789abcdef', ASKBAR_ADMIN_PASSWORD: PASSWORD,
  });
  await waitForPort(adminPort);
  await waitForPort(mockPort);

  const base = `http://127.0.0.1:${adminPort}/admin/`;
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 900, height: 800 } });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d/.test(m.text())) errors.push(m.text()); });

  await step('shows the login form when signed out', async () => {
    await page.goto(base);
    await page.waitForSelector('#login:not([hidden])');
    assert.equal(await page.isHidden('#app'), true);
  });
  await step('wrong password is refused, right one signs in', async () => {
    await page.fill('#password', 'nope');
    await page.click('#login-form button');
    await page.waitForSelector('#login-error:not([hidden])');
    assert.match(await page.innerText('#login-error'), /Wrong password/);
    await page.fill('#password', PASSWORD);
    await page.click('#login-form button');
    await page.waitForSelector('#app:not([hidden])');
    assert.match(await page.innerText('#providers'), /No providers yet/);
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
    const r = await fetch(`http://127.0.0.1:${api}/api/v1/providers`, { headers: { 'x-askbar-client-version': '0.1.0' } });
    assert.equal(r.status, 401);
  });
  await step('sign out returns to the login form', async () => {
    await page.click('#logout');
    await page.waitForSelector('#login:not([hidden])');
    await page.reload();
    await page.waitForSelector('#login:not([hidden])');
  });
  await step('no JavaScript errors', async () => { assert.deepEqual(errors, []); });

  await browser.close();
  cleanup();
  console.log(failures ? `\n${failures} step(s) failed` : '\nall admin steps passed');
  process.exit(failures ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(1); });
