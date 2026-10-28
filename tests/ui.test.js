// Run: node tests/ui.test.js   (needs `playwright` with Chromium installed)
// Drives the real ui/ files against tests/mock-tauri.js.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');

const ROOT = path.resolve(__dirname, '..');
const SHOTS = process.env.SHOT_DIR || null;

let failures = 0;
async function step(name, fn) {
  try { await fn(); console.log('ok   ' + name); }
  catch (e) { failures++; console.error('FAIL ' + name + '\n     ' + String(e.message).split('\n').join('\n     ')); }
}

async function open(browser, opts = {}) {
  const ctx = await browser.newContext({
    viewport: { width: 900, height: 760 },
    colorScheme: opts.colorScheme || 'light',
  });
  await ctx.addInitScript(`window.__opts = ${JSON.stringify(opts)};`);
  await ctx.addInitScript({ path: path.join(ROOT, 'tests/mock-tauri.js') });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
  await page.goto('file://' + path.join(ROOT, 'ui/index.html'));
  await page.addStyleTag({ content: 'html{background:#7d8088 !important}' }); // stand-in desktop for screenshots
  await page.waitForFunction(() => window.__calls.some((c) => c[0] === 'list_conversations'));
  page.errors = errors;
  return page;
}

const calls = (page, cmd) => page.evaluate((c) => window.__calls.filter((x) => x[0] === c).map((x) => x[1]), cmd);
const shot = async (page, name) => { if (SHOTS) await page.screenshot({ path: path.join(SHOTS, name + '.png') }); };

async function ask(page, text) {
  await page.fill('#input', text);
  await page.press('#input', 'Enter');
}
async function waitIdle(page) {
  await page.waitForFunction(() => document.getElementById('stop').hidden && !document.querySelector('.msg.streaming'));
}

(async () => {
  const browser = await chromium.launch();

  // ------------------------------------------------------------ basics
  let page = await open(browser);

  await step('starts as a bare input bar with focus', async () => {
    assert.equal(await page.isHidden('#thread'), true);
    assert.equal(await page.isHidden('#foot'), true);
    assert.equal(await page.isHidden('#history'), true);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'input');
    await shot(page, '01-idle');
  });

  await step('window is sized to the card plus the shadow margin', async () => {
    const r = await calls(page, 'resize_window');
    const last = r[r.length - 1];
    const card = await page.evaluate(() => { const c = document.getElementById('card'); return [c.offsetWidth, c.offsetHeight]; });
    assert.equal(last.width, card[0] + 64);
    assert.equal(last.height, card[1] + 64);
    assert.equal(card[0], 720);
  });

  await step('Shift+Enter inserts a newline and does not send', async () => {
    await page.fill('#input', 'line one');
    await page.press('#input', 'Shift+Enter');
    await page.type('#input', 'line two');
    assert.equal(await page.inputValue('#input'), 'line one\nline two');
    assert.equal((await calls(page, 'ask')).length, 0);
    await page.fill('#input', '');
  });

  await step('Enter sends; reply streams then settles', async () => {
    await ask(page, 'How do I format strings?');
    await page.waitForSelector('.msg.assistant.streaming');
    await shot(page, '02-streaming');
    await waitIdle(page);
    assert.equal(await page.inputValue('#input'), '', 'input cleared');
    assert.equal(await page.locator('.msg.user .body').innerText(), 'How do I format strings?');
    const html = await page.locator('.msg.assistant .body').innerHTML();
    assert.match(html, /<strong>format strings<\/strong>/);
    assert.match(html, /<table>/);
    assert.match(html, /class="copy-code"/);
    assert.equal(await page.isVisible('#foot'), true);
    const asks = await calls(page, 'ask');
    assert.equal(asks[0].conversationId, null);
    await shot(page, '03-answer');
  });

  await step('follow-up reuses the conversation id', async () => {
    await ask(page, 'And with width?');
    await waitIdle(page);
    const asks = await calls(page, 'ask');
    assert.equal(asks[1].conversationId, 1);
    assert.equal(await page.locator('.msg.user').count(), 2);
  });

  // -------------------------------------------------------------- copy
  await step('copy out: whole reply, code block, and last-reply shortcut', async () => {
    await page.evaluate(() => {
      window.__copied = [];
      Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (t) => { window.__copied.push(t); } }, configurable: true });
    });
    const reply = page.locator('.msg.assistant').first();
    await reply.hover();
    await reply.locator('.copy-msg').click();
    let copied = await page.evaluate(() => window.__copied);
    assert.ok(copied[0].startsWith('Sure. Use **format strings**'), 'copies the raw markdown source');
    assert.equal(await reply.locator('.copy-msg').innerText(), 'Copied');

    await reply.locator('.copy-code').click();
    copied = await page.evaluate(() => window.__copied);
    assert.equal(copied[1], 'name = "Ada"\nprint(f"Hello, {name}!")');

    await page.press('#input', 'Control+Shift+C');
    copied = await page.evaluate(() => window.__copied);
    assert.equal(copied.length, 3);
    assert.ok(copied[2].includes('verbose'));
  });

  await step('copy in: pasted multi-line text lands in the input and grows it', async () => {
    const before = await page.evaluate(() => document.getElementById('input').offsetHeight);
    await page.focus('#input');
    await page.evaluate(() => document.execCommand('insertText', false, 'a\nb\nc\nd'));
    const after = await page.evaluate(() => document.getElementById('input').offsetHeight);
    assert.ok(after > before, `${after} > ${before}`);
    assert.equal(await page.inputValue('#input'), 'a\nb\nc\nd');
    await page.fill('#input', '');
    await page.evaluate(() => document.getElementById('input').dispatchEvent(new Event('input')));
  });

  await step('links open through the backend, not the webview', async () => {
    await page.locator('.msg.assistant a').first().click();
    const opens = await calls(page, 'open_url');
    assert.deepEqual(opens[0], { url: 'https://example.com/docs' });
    assert.ok(page.url().endsWith('ui/index.html'), 'page did not navigate');
  });

  // ----------------------------------------------------------- history
  await step('Ctrl+H toggles the history panel', async () => {
    await page.press('#input', 'Control+h');
    assert.equal(await page.isVisible('#history'), true);
    assert.equal(await page.getAttribute('#history-toggle', 'aria-pressed'), 'true');
    assert.equal(await page.locator('.conv').count(), 1);
    assert.equal(await page.locator('.conv .open').innerText(), 'How do I format strings?');
    assert.equal(await page.locator('.history-group').innerText(), 'Today');
    const card = await page.evaluate(() => document.getElementById('card').offsetWidth);
    assert.equal(card, 720 + 260);
    await shot(page, '04-history');
    await page.press('#input', 'Control+h');
    assert.equal(await page.isHidden('#history'), true);
    assert.equal(await page.evaluate(() => document.getElementById('card').offsetWidth), 720);
  });

  await step('Ctrl+N starts a fresh conversation; history lists both', async () => {
    await page.press('#input', 'Control+n');
    assert.equal(await page.locator('.msg').count(), 0);
    assert.equal(await page.isHidden('#thread'), true);
    await ask(page, 'Second topic');
    await waitIdle(page);
    const asks = await calls(page, 'ask');
    assert.equal(asks[2].conversationId, null);
    await page.click('#history-toggle');
    assert.equal(await page.locator('.conv').count(), 2);
    assert.equal(await page.locator('.conv .open').first().innerText(), 'Second topic', 'most recent first');
    assert.equal(await page.locator('.conv.active').count(), 1);
  });

  await step('clicking a conversation loads it', async () => {
    await page.locator('.conv .open', { hasText: 'How do I format strings?' }).click();
    assert.equal(await page.locator('.msg.user').count(), 2);
    assert.equal(await page.locator('.msg.user .body').first().innerText(), 'How do I format strings?');
    assert.match(await page.locator('.conv.active .open').innerText(), /format strings/);
    // and continuing it appends to that conversation
    await ask(page, 'one more');
    await waitIdle(page);
    const asks = await calls(page, 'ask');
    assert.equal(asks[3].conversationId, 1);
  });

  await step('delete needs a second click and disarms on its own', async () => {
    const row = page.locator('.conv', { hasText: 'Second topic' });
    await row.hover();
    await row.locator('.del').click();
    assert.equal(await row.locator('.del').innerText(), 'Delete?');
    assert.equal(await page.locator('.conv').count(), 2, 'nothing deleted yet');
    await page.waitForTimeout(3200);
    assert.equal(await page.locator('.conv', { hasText: 'Second topic' }).locator('.del').innerText(), 'Delete');
    assert.equal(await page.locator('.conv').count(), 2);
  });

  await step('confirmed delete removes it; deleting the open one clears the thread', async () => {
    const other = page.locator('.conv', { hasText: 'Second topic' });
    await other.hover();
    await other.locator('.del').click();
    await other.locator('.del').click();
    await page.waitForFunction(() => document.querySelectorAll('.conv').length === 1);
    assert.equal(await page.locator('.msg.user').count(), 3, 'open conversation untouched');

    const open = page.locator('.conv.active');
    await open.hover();
    await open.locator('.del').click();
    await open.locator('.del').click();
    await page.waitForFunction(() => document.querySelectorAll('.conv').length === 0);
    assert.equal(await page.locator('.msg').count(), 0);
    assert.equal(await page.isHidden('#thread'), true);
    assert.match(await page.locator('.history-empty').innerText(), /No saved conversations/);
    const del = await calls(page, 'delete_conversation');
    assert.deepEqual(del.map((d) => d.id).sort(), [1, 2]);
    await shot(page, '05-history-empty');
  });

  await step('history groups by Today / Yesterday / Previous 7 days / Older', async () => {
    await page.evaluate(() => {
      const now = Math.floor(Date.now() / 1000);
      const mk = (id, title, ago) => ({ id, title, updated_at: now - ago, messages: [] });
      window.__db.convs.push(mk(11, 'two days ago', 2 * 86400 + 60), mk(12, 'long ago', 40 * 86400), mk(13, 'yesterday-ish', 86400 + 3600 * 12));
      window.__db.nextConv = 20;
    });
    await page.click('#history-toggle'); // close
    await page.click('#history-toggle'); // reopen -> refreshes
    const groups = await page.locator('.history-group').allInnerTexts();
    assert.ok(groups.includes('Older'), groups.join());
    assert.ok(groups.includes('Previous 7 days'), groups.join());
    const order = await page.locator('.conv .open').allInnerTexts();
    assert.equal(order[order.length - 1], 'long ago', 'oldest last');
    await page.evaluate(() => { window.__db.convs = window.__db.convs.filter((c) => c.id < 11); });
    await page.click('#history-toggle'); await page.click('#history-toggle');
  });

  // ------------------------------------------------------- stop & Esc
  await step('Esc stops a streaming reply (keeps partial text); next Esc hides', async () => {
    await page.click('#history-toggle'); // close history
    await ask(page, 'slow please');
    await page.waitForSelector('#stop:not([hidden])');
    await page.waitForFunction(() => document.querySelector('.msg.assistant .body').textContent.length > 20);
    await page.keyboard.press('Escape');
    await waitIdle(page);
    assert.equal((await calls(page, 'cancel_ask')).length, 1);
    assert.equal((await calls(page, 'hide_window')).length, 0, 'first Esc must not hide the window');
    const partial = await page.locator('.msg.assistant .body').last().innerText();
    assert.ok(partial.length > 10 && partial.length < 1000, `partial text kept (${partial.length} chars)`);
    await page.keyboard.press('Escape');
    assert.equal((await calls(page, 'hide_window')).length, 1);
  });

  await step('the Stop button works too', async () => {
    await ask(page, 'slow again');
    await page.waitForSelector('#stop:not([hidden])');
    await page.click('#stop');
    await waitIdle(page);
    assert.equal((await calls(page, 'cancel_ask')).length, 2);
  });

  // ------------------------------------------------------------ errors
  await step('errors show inline with a way to open the config', async () => {
    await page.keyboard.press('Control+n');
    await ask(page, 'please fail');
    await page.waitForSelector('.msg.error');
    assert.equal(await page.locator('.msg.assistant').count(), 0, 'empty reply bubble removed');
    assert.match(await page.locator('.msg.error .body').innerText(), /invalid x-api-key/);
    await shot(page, '06-error');
    await page.locator('.msg.error .open-config').click();
    assert.equal((await calls(page, 'open_config')).length, 1);
  });

  await step('input stays usable after an error', async () => {
    await ask(page, 'works now');
    await waitIdle(page);
    assert.match(await page.locator('.msg.assistant').last().innerText(), /format strings/);
  });

  await step('no JavaScript errors were logged', async () => {
    assert.deepEqual(page.errors, []);
  });
  await page.context().close();

  // ------------------------------------------------- notice + dark theme
  page = await open(browser, { notice: 'Welcome. Add your API key in /home/u/.config/askbar/config.yaml, then ask away.', theme: 'dark' });
  await step('first-run notice appears, offers the config, and dismisses', async () => {
    assert.equal(await page.isVisible('#notice'), true);
    assert.match(await page.innerText('#notice-text'), /Add your API key/);
    assert.equal(await page.isVisible('#notice-config'), true);
    await shot(page, '07-notice-dark');
    await page.click('#notice-close');
    assert.equal(await page.isHidden('#notice'), true);
  });

  await step('dark theme applies', async () => {
    assert.equal(await page.getAttribute('html', 'data-theme'), 'dark');
    const bg = await page.evaluate(() => getComputedStyle(document.getElementById('card')).backgroundColor);
    assert.equal(bg, 'rgb(27, 29, 35)');
    await ask(page, 'dark mode check');
    await waitIdle(page);
    await page.click('#history-toggle');
    await shot(page, '08-dark-history');
  });

  await step('re-showing the window (event) re-selects the draft and refreshes settings', async () => {
    await page.fill('#input', 'half typed');
    await page.evaluate(() => window.__listeners['askbar:shown']());
    await page.waitForTimeout(100);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'input');
    assert.equal(await page.evaluate(() => { const i = document.getElementById('input'); return i.selectionStart === 0 && i.selectionEnd === i.value.length; }), true);
  });
  await page.context().close();

  // ------------------------------------------------- server states
  const STATES = {
    offline: { kind: 'offline', message: 'Server offline. Could not reach http://localhost:8080. Check that it is running and that you are on the right network.' },
    rejected: { kind: 'rejected', message: 'Token rejected. The server no longer accepts this device; it may have been revoked.' },
    update_needed: { kind: 'update_needed', message: 'Update needed. This version of Askbar is too old for the server.' },
    provider_missing: { kind: 'provider_missing', message: 'Provider "gone" no longer exists on the server. Pick another.', providers: ['alpha', 'beta'] },
  };

  page = await open(browser);
  await step('no banner while the server is healthy', async () => {
    assert.equal(await page.isHidden('#status'), true);
  });
  await page.context().close();

  for (const [kind, status] of Object.entries(STATES)) {
    page = await open(browser, { status });
    await step(`${kind}: banner shows the message and a way forward when the window opens`, async () => {
      await page.waitForSelector('#status:not([hidden])');
      assert.equal(await page.getAttribute('#status', 'data-kind'), kind);
      assert.equal(await page.innerText('#status-text'), status.message);
      assert.equal(await page.isVisible('#status-actions [data-action="retry"]'), true);
      await shot(page, `09-status-${kind}`);
    });
    if (kind !== 'provider_missing') {
      await step(`${kind}: sending fails visibly, and Retry clears the banner once the server is back`, async () => {
        await ask(page, 'hello');
        await waitIdle(page);
        assert.match(await page.innerText('.msg.error'), new RegExp(status.message.slice(0, 20)));
        await page.evaluate(() => { window.__opts.status = null; });
        await page.click('#status-actions [data-action="retry"]');
        await page.waitForSelector('#status', { state: 'hidden' });
        await ask(page, 'second try');
        await waitIdle(page);
        assert.equal(await page.locator('.msg.assistant').count() >= 1, true);
      });
    }
    await page.context().close();
  }

  page = await open(browser, { status: STATES.provider_missing });
  await step('provider_missing: picking another provider fixes it and updates the label', async () => {
    await page.waitForSelector('#status:not([hidden])');
    const labels = await page.locator('#status-actions [data-action="provider"]').allInnerTexts();
    assert.deepEqual(labels, ['Use alpha', 'Use beta']);
    await page.click('#status-actions [data-action="provider"] >> text=Use beta');
    await page.waitForSelector('#status', { state: 'hidden' });
    assert.deepEqual(await calls(page, 'set_active_provider'), [{ name: 'beta' }]);
    await ask(page, 'works now');
    await waitIdle(page);
    assert.match(await page.innerText('#model-label'), /via beta/);
  });
  await step('server_status is re-checked when the window is shown again', async () => {
    await page.evaluate(() => { window.__opts.status = { kind: 'offline', message: 'Server offline.' }; });
    await page.evaluate(() => window.__listeners['askbar:shown']());
    await page.waitForSelector('#status:not([hidden])');
    assert.equal(await page.getAttribute('#status', 'data-kind'), 'offline');
  });
  await step('no JavaScript errors in the status screens', async () => {
    assert.deepEqual(page.errors, []);
  });
  await page.context().close();

  await browser.close();
  console.log(failures ? `\n${failures} step(s) failed` : '\nall UI steps passed');
  process.exit(failures ? 1 : 0);
})();
