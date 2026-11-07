// Run (from clients/desktop): node tests/ui.test.js   (needs `playwright` with Chromium installed)
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
    assert.equal(asks[1].conversationId, '1');
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
    assert.equal(asks[3].conversationId, '1');
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
    assert.deepEqual(del.map((d) => d.id).sort(), ['1', '2']);
    await shot(page, '05-history-empty');
  });

  await step('history groups by Today / Yesterday / Previous 7 days / Older', async () => {
    await page.evaluate(() => {
      const now = Math.floor(Date.now() / 1000);
      const mk = (id, title, ago) => ({ id, title, updated_at: now - ago, messages: [] });
      window.__db.convs.push(mk('11', 'two days ago', 2 * 86400 + 60), mk('12', 'long ago', 40 * 86400), mk('13', 'yesterday-ish', 86400 + 3600 * 12));
      window.__db.nextConv = 20;
    });
    await page.click('#history-toggle'); // close
    await page.click('#history-toggle'); // reopen -> refreshes
    const groups = await page.locator('.history-group').allInnerTexts();
    assert.ok(groups.includes('Older'), groups.join());
    assert.ok(groups.includes('Previous 7 days'), groups.join());
    const order = await page.locator('.conv .open').allInnerTexts();
    assert.equal(order[order.length - 1], 'long ago', 'oldest last');
    await page.evaluate(() => { window.__db.convs = window.__db.convs.filter((c) => Number(c.id) < 11); });
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

  // ------------------------------------------------- replies still running on the server
  {
    const p2 = await open(browser);
    await p2.evaluate(() => {
      window.__db.convs.push({
        id: '7', title: 'Long question', updated_at: 999, messages: [{ id: 'm1', role: 'user', content: 'Long question', created_at: 1 }],
        pending: [
          { generationId: 'g1', status: 'running', prompt: 'Long question', replyText: 'Running reply text from the server. '.repeat(3), slow: true },
          { generationId: 'g2', status: 'queued', prompt: 'Waiting question', replyText: 'Answer to the waiting question.' },
          { generationId: 'g3', status: 'queued', prompt: 'Unwanted question' },
        ],
      });
      window.__db.nextConv = 8;
    });

    await step('opening a conversation joins its running reply and lists the waiting messages', async () => {
      await p2.click('#history-toggle');
      await p2.locator('.conv .open', { hasText: 'Long question' }).click();
      await p2.waitForSelector('#stop:not([hidden])');
      assert.deepEqual((await calls(p2, 'attach')).map((a) => a.generationId), ['g1']);
      assert.deepEqual(await calls(p2, 'load_pending'), [{ id: '7' }].concat((await calls(p2, 'load_pending')).slice(1)));
      await p2.waitForFunction(() => document.querySelector('.msg.assistant .body').textContent.includes('Running reply text'));
      const rows = p2.locator('.msg.waiting');
      assert.equal(await rows.count(), 2);
      assert.equal(await rows.nth(0).locator('.body').innerText(), 'Waiting question');
      assert.match(await rows.nth(0).innerText(), /Waiting/);
      assert.equal(await p2.locator('.del-queued').count(), 2, 'each waiting message has a way to remove it');
      await shot(p2, '20-waiting');
    });

    await step('removing a waiting message cancels it and takes the row away', async () => {
      await p2.locator('.msg.waiting', { hasText: 'Unwanted question' }).locator('.del-queued').click();
      await p2.waitForFunction(() => document.querySelectorAll('.msg.waiting').length === 1);
      assert.deepEqual(await calls(p2, 'cancel_generation'), [{ id: 'g3' }]);
      assert.equal(await p2.locator('.msg.waiting .body').innerText(), 'Waiting question');
    });

    await step('when the reply ends, the waiting message starts and its reply is joined', async () => {
      await p2.waitForFunction(() => (window.__calls.filter((c) => c[0] === 'attach').length === 2), null, { timeout: 15000 });
      await p2.waitForFunction(() => document.querySelectorAll('.msg.waiting').length === 0);
      await waitIdle(p2);
      assert.deepEqual((await calls(p2, 'attach')).map((a) => a.generationId), ['g1', 'g2']);
      const bodies = await p2.locator('.msg .body').allInnerTexts();
      assert.ok(bodies.some((b) => b.includes('Answer to the waiting question.')), bodies.join(' | '));
      assert.ok(bodies.some((b) => b === 'Waiting question'), 'its question is now a normal message');
    });

    await step('re-showing the window picks up a reply that started elsewhere', async () => {
      await p2.evaluate(() => {
        const c = window.__db.convs.find((x) => x.id === '7');
        c.pending = [{ generationId: 'g9', status: 'running', prompt: 'from elsewhere', replyText: 'Started on another app.' }];
        window.__listeners['askbar:shown']();
      });
      await p2.waitForFunction(() => window.__calls.some((c) => c[0] === 'attach' && c[1].generationId === 'g9'));
      await waitIdle(p2);
      assert.ok((await p2.locator('.msg.assistant .body').last().innerText()).includes('Started on another app.'));
    });

    await step('no JavaScript errors while joining running replies', async () => {
      assert.deepEqual(p2.errors, []);
    });
    await p2.context().close();
  }

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

  await step('a timeout error after two deltas keeps the text shown and adds the message', async () => {
    await page.keyboard.press('Control+n');
    await ask(page, 'this will timeout');
    await page.waitForSelector('.msg.error');
    await waitIdle(page);
    assert.equal(await page.locator('.msg.assistant .body').innerText(), 'Partial answer', 'the reply stays');
    assert.equal(await page.locator('.msg.error .body').innerText(), 'Stopped after 5 minutes');
    assert.equal(await page.locator('.msg.assistant').count(), 1, 'one reply bubble, not removed');
    assert.equal(await page.isVisible('#input'), true);
  });

  await step('Try again after a timeout asks the same question as a new reply and keeps the partial one', async () => {
    const before = (await calls(page, 'ask')).length;
    await page.locator('.msg.error .retry-reply').click();
    await waitIdle(page);
    const asks = await calls(page, 'ask');
    assert.equal(asks.length, before + 1);
    assert.equal(asks[asks.length - 1].text, 'this will timeout');
    assert.equal(await page.locator('.msg.assistant').count(), 2, 'the partial reply stays and a new one begins');
    assert.equal(await page.locator('.msg.assistant .body').first().innerText(), 'Partial answer');
  });

  await step('a provider event never adds an "Answered by" line (there is no fallback)', async () => {
    await page.keyboard.press('Control+n');
    await ask(page, 'announce another provider please');
    await waitIdle(page);
    assert.equal(await page.locator('.msg.assistant').count(), 1);
    assert.doesNotMatch(await page.locator('.msg.assistant').innerText(), /Answered by/);
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

  // ------------------------------------------------- server defaults and local overrides
  page = await open(browser, { serverUi: { theme: 'dark', width: 900 } });
  await step('the look follows the server default for this user', async () => {
    assert.equal(await page.getAttribute('html', 'data-theme'), 'dark');
    assert.equal(await page.evaluate(() => document.documentElement.style.getPropertyValue('--card-w')), '900px');
  });
  await step('a local override wins, for its own field only', async () => {
    await page.evaluate(() => { window.__opts.override = { theme: 'light' }; });
    await page.evaluate(() => window.__listeners['askbar:shown']());
    await page.waitForFunction(() => document.documentElement.dataset.theme === 'light');
    assert.equal(await page.evaluate(() => document.documentElement.style.getPropertyValue('--card-w')), '900px', 'width still follows the server');
  });
  await step('resetting the override returns the server default', async () => {
    await page.evaluate(() => { window.__opts.override = null; });
    await page.evaluate(() => window.__listeners['askbar:shown']());
    await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
    assert.equal(await page.evaluate(() => document.documentElement.style.getPropertyValue('--card-w')), '900px');
  });
  await step('no JavaScript errors with server defaults and overrides', async () => {
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
    rejected: { kind: 'rejected', message: 'Token rejected. The server no longer accepts this app; it may have been revoked.' },
    update_needed: { kind: 'update_needed', message: 'Update needed. This version of Askbar is too old for the server.' },
    provider_missing: { kind: 'provider_missing', message: 'The provider you picked no longer exists on the server. Pick another.', providers: [{ id: 'p1', name: 'alpha' }, { id: 'p2', name: 'beta' }] },
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

  // ---------------------------------------------- the outbox: messages the server never took
  {
    const p3 = await open(browser, { status: STATES.offline });
    await step('offline: a failed send moves to the unsent list with Send, Edit and Discard, and is not retried by itself', async () => {
      await p3.waitForSelector('#status:not([hidden])');
      await ask(p3, 'please keep this');
      await p3.waitForSelector('.msg.unsent');
      await waitIdle(p3);
      assert.equal(await p3.locator('.msg.unsent .body').innerText(), 'please keep this');
      assert.equal(await p3.locator('.msg.user:not(.unsent)').count(), 0, 'not shown as if it had been sent');
      for (const [cls, label] of [['unsent-send', 'Send'], ['unsent-edit', 'Edit'], ['unsent-discard', 'Discard']]) {
        assert.equal(await p3.locator(`.msg.unsent .${cls}`).innerText(), label);
      }
      assert.match(await p3.innerText('#status-text'), /Server offline/, 'the offline message stays');
      await p3.waitForTimeout(400);
      assert.equal((await calls(p3, 'ask')).length, 1, 'asked once');
      assert.equal((await calls(p3, 'resend')).length, 0, 'nothing is sent automatically');
      await shot(p3, '21-unsent');
    });

    await step('Edit puts the text back in the input and removes the unsent entry', async () => {
      await p3.locator('.msg.unsent .unsent-edit').click();
      await p3.waitForFunction(() => document.querySelectorAll('.msg.unsent').length === 0);
      assert.equal(await p3.inputValue('#input'), 'please keep this');
      assert.equal((await calls(p3, 'discard_unsent')).length, 1);
      assert.equal((await calls(p3, 'unsent_messages')).length >= 1, true);
      await p3.fill('#input', '');
    });

    await step('Discard removes the entry without sending it', async () => {
      await ask(p3, 'throw this away');
      await p3.waitForSelector('.msg.unsent');
      await p3.locator('.msg.unsent .unsent-discard').click();
      await p3.waitForFunction(() => document.querySelectorAll('.msg.unsent').length === 0);
      assert.equal(await p3.evaluate(() => window.__db.unsent.length), 0);
      assert.equal((await calls(p3, 'resend')).length, 0);
      assert.equal(await p3.inputValue('#input'), '');
    });

    await step('Send, once the server is back, sends that message again and the reply streams', async () => {
      await ask(p3, 'send me later');
      await p3.waitForSelector('.msg.unsent');
      await p3.evaluate(() => { window.__opts.status = null; });
      await p3.locator('.msg.unsent .unsent-send').click();
      await waitIdle(p3);
      const r = await calls(p3, 'resend');
      assert.equal(r.length, 1);
      assert.match(r[0].clientRequestId, /^req-/);
      assert.equal(await p3.locator('.msg.unsent').count(), 0);
      assert.equal(await p3.locator('.msg.user .body').last().innerText(), 'send me later');
      assert.match(await p3.locator('.msg.assistant').last().innerText(), /format strings/);
    });

    await step('unsent messages come back when the window is shown again', async () => {
      await p3.evaluate(() => {
        window.__db.unsent.push({ clientRequestId: 'req-x', conversationId: null, text: 'left over', createdAt: 1 });
      });
      await p3.keyboard.press('Control+n');
      await p3.waitForSelector('.msg.unsent');
      assert.equal(await p3.locator('.msg.unsent .body').innerText(), 'left over');
    });

    await step('no JavaScript errors in the outbox flow', async () => {
      assert.deepEqual(p3.errors, []);
    });
    await p3.context().close();
  }

  page = await open(browser, { status: STATES.provider_missing });
  await step('provider_missing: picking another provider fixes it and updates the label', async () => {
    await page.waitForSelector('#status:not([hidden])');
    const labels = await page.locator('#status-actions [data-action="provider"]').allInnerTexts();
    assert.deepEqual(labels, ['Use alpha', 'Use beta']);
    await page.click('#status-actions [data-action="provider"] >> text=Use beta');
    await page.waitForSelector('#status', { state: 'hidden' });
    assert.deepEqual(await calls(page, 'set_active_provider'), [{ id: 'p2' }]);
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

  // ------------------------------------------------- pairing screen
  page = await open(browser, { status: { kind: 'not_paired', message: 'This app is not paired with the server yet.' } });
  await step('not paired: the banner offers the pairing screen', async () => {
    await page.waitForSelector('#status:not([hidden])');
    assert.equal(await page.isVisible('#status-actions [data-action="pair"]'), true);
    assert.equal(await page.isHidden('#pair'), true);
    await page.click('#status-actions [data-action="pair"]');
    assert.equal(await page.isVisible('#pair'), true);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'pair-input');
    await shot(page, '10-pairing');
  });
  await step('a bad pairing key shows the reason and stays on the screen', async () => {
    await page.fill('#pair-input', 'hello');
    await page.click('#pair-submit');
    await page.waitForSelector('#pair-error:not([hidden])');
    assert.match(await page.innerText('#pair-error'), /not a pairing key/);
    await page.fill('#pair-input', 'askbar-pair:bad');
    await page.click('#pair-submit');
    await page.waitForFunction(() => /wrong, expired/.test(document.getElementById('pair-error').textContent));
    assert.equal(await page.isVisible('#pair'), true);
  });
  await step('Esc closes the pairing screen without hiding the window', async () => {
    await page.press('#pair-input', 'Escape');
    assert.equal(await page.isHidden('#pair'), true);
    assert.equal((await calls(page, 'hide_window')).length, 0);
    await page.evaluate(() => window.__listeners['askbar:pair']());
    assert.equal(await page.isVisible('#pair'), true);
  });
  await step('a good pairing key pairs, clears the banner and lets you ask', async () => {
    await page.fill('#pair-input', 'askbar-pair:abc');
    await page.press('#pair-input', 'Enter');
    await page.waitForSelector('#pair', { state: 'hidden' });
    await page.waitForSelector('#status', { state: 'hidden' });
    assert.deepEqual((await calls(page, 'pair')).pop(), { pairing: 'askbar-pair:abc' });
    await ask(page, 'hello after pairing');
    await waitIdle(page);
    assert.equal(await page.locator('.msg.assistant').count(), 1);
  });
  await step('Pair another app makes a key to copy, shows a refusal, and Esc closes it', async () => {
    await page.evaluate(() => window.__listeners['askbar:pair-another']());
    assert.equal(await page.isVisible('#share'), true);
    assert.equal(await page.isHidden('#share-out'), true);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'share-name');
    await page.fill('#share-name', 'fail');
    await page.click('#share-submit');
    await page.waitForSelector('#share-error:not([hidden])');
    assert.match(await page.innerText('#share-error'), /Too many/);
    await page.fill('#share-name', 'Work laptop');
    await page.click('#share-submit');
    await page.waitForSelector('#share-out:not([hidden])');
    assert.equal(await page.inputValue('#share-key'), 'askbar-pair:shared-key');
    assert.equal(await page.isHidden('#share-error'), true);
    assert.deepEqual((await calls(page, 'new_pairing_key')).pop(), { name: 'Work laptop' });
    await shot(page, '11-pair-another');
    await page.press('#share-name', 'Escape');
    assert.equal(await page.isHidden('#share'), true);
    assert.equal(await page.inputValue('#share-key'), '');
  });
  await step('rejected token also offers the pairing screen', async () => {
    await page.evaluate(() => { window.__opts.status = { kind: 'rejected', message: 'Token rejected.' }; });
    await page.evaluate(() => window.__listeners['askbar:shown']());
    await page.waitForSelector('#status-actions [data-action="pair"]');
  });
  await step('no JavaScript errors on the pairing screen', async () => {
    assert.deepEqual(page.errors, []);
  });
  await page.context().close();

  await browser.close();
  console.log(failures ? `\n${failures} step(s) failed` : '\nall UI steps passed');
  process.exit(failures ? 1 : 0);
})();
