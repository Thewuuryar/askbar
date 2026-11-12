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

  await step('starts as an input bar with its footer, and focus', async () => {
    assert.equal(await page.isHidden('#thread'), true);
    assert.equal(await page.isVisible('#foot'), true, 'the footer is always shown');
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

  await step('a link asks first: its full address is shown, and nothing opens until Open (D18)', async () => {
    await page.locator('.msg.assistant a').first().click();
    assert.equal(await page.isVisible('#link-confirm'), true);
    assert.equal(await page.innerText('#link-confirm-url'), 'https://example.com/docs');
    assert.equal((await calls(page, 'open_url')).length, 0, 'nothing opened yet');
    assert.ok(page.url().endsWith('ui/index.html'), 'the page did not navigate');
    await page.click('#link-open');
    assert.equal(await page.isHidden('#link-confirm'), true);
    const opens = await calls(page, 'open_url');
    assert.deepEqual(opens, [{ url: 'https://example.com/docs' }], 'opened through the backend, once');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'input');
  });

  await step('Cancel and Esc close the bar and open nothing, and Esc does not also hide the window', async () => {
    const hides = (await calls(page, 'hide_window')).length;
    await page.locator('.msg.assistant a').first().click();
    await page.click('#link-cancel');
    assert.equal(await page.isHidden('#link-confirm'), true);
    await page.locator('.msg.assistant a').first().click();
    assert.equal(await page.isVisible('#link-confirm'), true);
    await page.keyboard.press('Escape');
    assert.equal(await page.isHidden('#link-confirm'), true);
    assert.equal((await calls(page, 'open_url')).length, 1, 'still only the one that was confirmed');
    assert.equal((await calls(page, 'hide_window')).length, hides, 'the first Esc closed the bar only');
  });

  await step('the bar shows where a link really goes, whatever its label says, and a very long address in full', async () => {
    const lp = await open(browser);
    const long = 'https://example.com/' + 'a'.repeat(1900);
    await lp.evaluate((r) => { window.__nextReply = r; }, `[https://bank.example/login](https://evil.example/phish?x=1&y=2) and [docs](${long})`);
    await lp.press('#input', 'Control+n');
    await ask(lp, 'links please');
    await waitIdle(lp);
    await lp.locator('.msg.assistant a', { hasText: 'https://bank.example/login' }).click();
    assert.equal(await lp.innerText('#link-confirm-url'), 'https://evil.example/phish?x=1&y=2', 'the address, not the label');
    await lp.click('#link-cancel');
    await lp.locator('.msg.assistant a', { hasText: 'docs' }).click();
    assert.equal(await lp.evaluate(() => document.getElementById('link-confirm-url').textContent), long, 'in full, not cut');
    assert.equal(await lp.isVisible('#link-open'), true, 'the buttons stay on screen');
    await lp.keyboard.press('Escape');
    await lp.context().close();
  });

  await step('a link that is not http or https never reaches the bar, and image syntax is a link, not a picture', async () => {
    const lp = await open(browser);
    await lp.evaluate((r) => { window.__nextReply = r; }, '<a href="javascript:window.__pwned=1">raw</a> [js](javascript:window.__pwned=1) ![pic](https://example.com/i.png)');
    await lp.press('#input', 'Control+n');
    await ask(lp, 'odd links');
    await waitIdle(lp);
    assert.equal(await lp.locator('.msg.assistant a').count(), 1, 'only the image-as-link is a link');
    assert.equal(await lp.locator('.msg.assistant img').count(), 0);
    await lp.locator('.msg.assistant a').click();
    assert.equal(await lp.innerText('#link-confirm-url'), 'https://example.com/i.png');
    await lp.keyboard.press('Escape');
    // Even a link made after the fact, with a scheme the renderer would never write, is refused before the bar.
    await lp.evaluate(() => {
      const a = document.createElement('a');
      a.setAttribute('href', 'javascript:window.__pwned=1');
      a.textContent = 'planted';
      document.querySelector('.msg.assistant .body').appendChild(a);
    });
    await lp.locator('.msg.assistant a', { hasText: 'planted' }).click();
    assert.equal(await lp.isHidden('#link-confirm'), true);
    assert.equal(await lp.evaluate(() => window.__pwned), undefined);
    await lp.context().close();
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

  await step('a question that got no reply is tinted, and its error line offers Edit and Try again', async () => {
    const q = page.locator('.msg.user');
    assert.equal(await q.count(), 1);
    assert.match(await q.getAttribute('class'), /\bfailed\b/);
    // The error line comes right after the question it belongs to.
    assert.equal(await q.evaluate((n) => n.nextElementSibling.matches('.msg.error')), true);
    const tint = await q.locator('.body').evaluate((n) => getComputedStyle(n).backgroundColor);
    assert.notEqual(tint, 'rgba(0, 0, 0, 0)', 'the question has a tint');

    await page.locator('.msg.error .edit-question').click();
    assert.equal(await page.inputValue('#input'), 'please fail', 'Edit puts the text back in the box');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'input');
    await page.fill('#input', '');

    const before = (await calls(page, 'ask')).length;
    await page.locator('.msg.error .retry-reply').click();
    await waitIdle(page);
    const asks = await calls(page, 'ask');
    assert.equal(asks.length, before + 1);
    assert.equal(asks[asks.length - 1].text, 'please fail');
    assert.deepEqual(await page.locator('.msg.user .body').allInnerTexts(), ['please fail', 'please fail'], 'asked again below');
    assert.equal(await page.locator('.msg.user.failed').count(), 2, 'the first stays marked: it did fail');
    assert.equal(await page.locator('.msg.error').count(), 2);
  });

  await step('after reopening, a failed question shows the same tint, line and buttons', async () => {
    await page.click('#history-toggle');
    await page.locator('.conv .open', { hasText: 'please fail' }).click();
    assert.equal(await page.locator('.msg.user.failed').count(), 2);
    const lines = page.locator('.msg.error');
    assert.deepEqual(await lines.locator('.body').allInnerTexts(), ['The provider could not answer.', 'The provider could not answer.'],
      "a reopened chat shows the server's sentence");
    assert.equal(await lines.first().locator('.retry-reply').count(), 1);
    assert.equal(await lines.first().locator('.edit-question').count(), 1);
    assert.equal(await page.locator('.msg.user').first().evaluate((n) => n.nextElementSibling.matches('.msg.error')), true);
    await page.click('#history-toggle');
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

  await step('a question cut off by a timeout is not tinted: its line follows the partial reply', async () => {
    assert.equal(await page.locator('.msg.user.failed').count(), 0, 'part of a reply came');
    assert.equal(await page.locator('.msg.assistant').evaluate((n) => n.nextElementSibling.matches('.msg.error')), true);
    assert.equal(await page.locator('.msg.error .edit-question').count(), 1);
  });

  await step('Try again after a timeout asks the same question again and keeps the partial reply', async () => {
    const before = (await calls(page, 'ask')).length;
    await page.locator('.msg.error .retry-reply').click();
    await waitIdle(page);
    const asks = await calls(page, 'ask');
    assert.equal(asks.length, before + 1);
    assert.equal(asks[asks.length - 1].text, 'this will timeout');
    assert.equal(await page.locator('.msg.assistant').count(), 2, 'the partial reply stays and a new one begins');
    assert.equal(await page.locator('.msg.assistant .body').first().innerText(), 'Partial answer');
    // The question is shown again, as the server stores it and a reopened chat shows it.
    assert.deepEqual(await page.locator('.msg.user .body').allInnerTexts(), ['this will timeout', 'this will timeout']);
  });

  await step('after reopening, the timed-out question keeps its line after the partial reply', async () => {
    await page.click('#history-toggle');
    await page.locator('.conv .open', { hasText: 'this will timeout' }).click();
    assert.equal(await page.locator('.msg.user.failed').count(), 0);
    const first = page.locator('.msg.assistant').first();
    assert.equal(await first.evaluate((n) => n.nextElementSibling.matches('.msg.error')), true);
    assert.equal(await page.locator('.msg.error .body').first().innerText(), 'Stopped after 5 minutes');
    await page.click('#history-toggle');
  });

  await step('the history panel copies a whole conversation as Markdown, with who, when, model and failures', async () => {
    await page.evaluate(() => { window.__copied = []; });
    await page.keyboard.press('Control+n');
    const shown = await page.locator('.msg').count();
    await page.click('#history-toggle');
    const row = page.locator('.conv', { hasText: 'this will timeout' });
    await row.hover();
    const btn = row.locator('.copy-conv');
    assert.equal(await btn.getAttribute('aria-label'), 'Copy conversation: this will timeout');
    await btn.click();
    await page.waitForFunction(() => window.__copied.length === 1);
    const md = (await page.evaluate(() => window.__copied))[0];
    const parts = md.trim().split('\n\n');
    assert.equal(parts[0], '# this will timeout');
    assert.match(parts[1], /^\*\*You\*\* · \S/);
    assert.equal(parts[2], 'this will timeout');
    assert.equal(parts[3], '> Failed: Stopped after 5 minutes', 'a failed question says why, under it');
    assert.match(parts[4], /^\*\*Assistant\*\* · .+ · mock-1 via Mock AI$/);
    assert.equal(parts[5], 'Partial answer');
    assert.equal(md.split('**You**').length - 1, 2, 'every question, in order');
    assert.equal(await btn.innerText(), 'Copied');
    // Copying does not change what the window shows.
    assert.equal(await page.locator('.msg').count(), shown);
    await page.click('#history-toggle');
  });

  await step('a provider event never adds an "Answered by" line (there is no fallback)', async () => {
    await page.keyboard.press('Control+n');
    await ask(page, 'announce another provider please');
    await waitIdle(page);
    assert.equal(await page.locator('.msg.assistant').count(), 1);
    assert.doesNotMatch(await page.locator('.msg.assistant').innerText(), /Answered by/);
  });

  await step('each message shows its time on hover, and a reply names its model and provider', async () => {
    // Plan step 1.13: the date and time under every message, and a chip beside a reply's time whose
    // tooltip names the model and provider that wrote it, live and after reopening.
    const reply = page.locator('.msg.assistant').last();
    const question = page.locator('.msg.user').last();
    const chip = reply.locator('.meta .by');
    assert.equal(await chip.getAttribute('title'), 'Model: backup-2 · Provider: Backup AI');
    assert.equal(await chip.getAttribute('aria-label'), 'Model: backup-2 · Provider: Backup AI');
    assert.equal(await question.locator('.by').count(), 0, 'a question names no provider');
    for (const m of [reply, question]) {
      const when = m.locator('.meta time.when');
      assert.match(await when.getAttribute('datetime'), /^\d{4}-\d\d-\d\dT/);
      assert.ok((await when.innerText()).trim().length > 0);
    }

    // Hidden until hover, and showing it moves nothing. (The row fades, so wait for it to settle.)
    const meta = reply.locator('.meta');
    const settlesAt = async (want) => {
      for (let i = 0; i < 50; i++) {
        if ((await meta.evaluate((e) => getComputedStyle(e).opacity)) === want) return;
        await page.waitForTimeout(20);
      }
      assert.fail('the meta row did not reach opacity ' + want);
    };
    await page.locator('#input').hover();
    await settlesAt('0');
    const before = await reply.boundingBox();
    const qBefore = await question.boundingBox();
    await reply.hover();
    await settlesAt('1');
    assert.deepEqual(await reply.boundingBox(), before, 'the reply does not move or grow');
    assert.deepEqual(await question.boundingBox(), qBefore, 'nothing above it moves');

    // Keyboard: the chip can take focus, which shows the row.
    await page.locator('#input').hover();
    await settlesAt('0');
    await chip.focus();
    await settlesAt('1');
    await page.locator('#input').focus();

    // After reopening, the stored reply keeps the same label.
    await page.click('#history-toggle');
    await page.locator('.conv .open', { hasText: 'announce another provider' }).click();
    assert.equal(await page.locator('.msg.assistant .by').last().getAttribute('title'), 'Model: backup-2 · Provider: Backup AI');
    await page.click('#history-toggle');
  });

  await step('a reply stored without a provider shows its time and no chip', async () => {
    await page.evaluate(() => {
      window.__db.convs.push({
        id: 'old',
        title: 'From before labels',
        updated_at: 1,
        messages: [
          { id: 'o1', role: 'user', content: 'old question', created_at: 1700000000 },
          { id: 'o2', role: 'assistant', content: 'old answer', created_at: 1700000060 },
        ],
      });
    });
    await page.click('#history-toggle');
    await page.locator('.conv .open', { hasText: 'From before labels' }).click();
    const old = page.locator('.msg.assistant').last();
    assert.equal(await old.locator('.by').count(), 0);
    assert.equal(await old.locator('time.when').getAttribute('datetime'), new Date(1700000060 * 1000).toISOString());
    await page.click('#history-toggle');
    await page.keyboard.press('Control+n');
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

  // ------------------------------------------------- newest first
  page = await open(browser, { serverUi: { newest_first: true } });
  // The top edge of each row, in the order the rows are drawn.
  const drawn = (p) => p.evaluate(() => [...document.querySelectorAll('#thread > .msg')]
    .map((n) => ({ y: n.getBoundingClientRect().top, cls: n.className, text: n.querySelector('.body').innerText }))
    .sort((a, b) => a.y - b.y));
  await step('newest first: the latest turn is under the bar, each question above its reply', async () => {
    assert.equal(await page.getAttribute('#thread', 'data-order'), 'newest');
    for (const q of ['First question', 'Second question']) {
      await ask(page, q);
      await waitIdle(page);
    }
    await ask(page, 'Third, please fail');
    await page.waitForSelector('.msg.error');
    await waitIdle(page);
    const rows = await drawn(page);
    const kinds = rows.map((r) => (r.cls.includes('user') ? 'Q:' + r.text : r.cls.includes('error') ? 'error' : 'reply'));
    assert.deepEqual(kinds, ['Q:Third, please fail', 'error', 'Q:Second question', 'reply', 'Q:First question', 'reply']);
    assert.equal(await page.evaluate(() => document.getElementById('thread').scrollTop), 0, 'the newest stays in view');
    // The rows themselves stay in time order: copying and the shortcuts read them as before.
    const dom = await page.evaluate(() => [...document.querySelectorAll('#thread > .msg.user .body')].map((n) => n.innerText));
    assert.deepEqual(dom, ['First question', 'Second question', 'Third, please fail']);
  });
  await step('newest first holds after reopening', async () => {
    await page.click('#history-toggle');
    await page.locator('.conv .open', { hasText: 'First question' }).click();
    await page.click('#history-toggle');
    const rows = await drawn(page);
    assert.equal(rows[0].text, 'Third, please fail');
    assert.equal(rows[rows.length - 1].cls.includes('assistant'), true, 'the oldest reply is last');
  });
  await step('a local override of the order wins over the server default', async () => {
    await page.evaluate(() => { window.__opts.override = { newest_first: false }; });
    await page.evaluate(() => window.__listeners['askbar:shown']());
    await page.waitForFunction(() => document.getElementById('thread').dataset.order === 'oldest');
    const rows = await drawn(page);
    assert.equal(rows[0].text, 'First question', 'oldest first again');
  });
  await step('no JavaScript errors with newest first', async () => {
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

  // ------------------------------------------------- the model picker (ADR 0014)
  {
    const models = [
      { id: 'p1/gpt-x', label: 'gpt-x via OpenAI' },
      { id: 'p1/gpt-y', label: 'gpt-y via OpenAI' },
      { id: 'p2/claude-z', label: 'claude-z via Anthropic' },
    ];
    page = await open(browser, { models, activeModel: 'p1/gpt-x' });
    await step('several approved models: a picker shows them, with the one in use selected; the plain label is hidden', async () => {
      await page.waitForSelector('#model-picker:not([hidden])');
      assert.equal(await page.isVisible('#foot'), true, 'the footer is always shown, so the model can be picked before the first question');
      assert.deepEqual(await page.$$eval('#model-picker option', (o) => o.map((x) => x.value)), ['p1/gpt-x', 'p1/gpt-y', 'p2/claude-z']);
      assert.equal(await page.inputValue('#model-picker'), 'p1/gpt-x');
      assert.equal(await page.isHidden('#model-label'), true);
    });
    await step('picking a model is remembered for new chats, and the next send names it', async () => {
      await page.selectOption('#model-picker', 'p2/claude-z');
      assert.deepEqual(await calls(page, 'set_active_model'), [{ id: 'p2/claude-z' }]);
      await ask(page, 'first question');
      await waitIdle(page);
      const asks = await calls(page, 'ask');
      assert.equal(asks[asks.length - 1].model, 'p2/claude-z');
    });
    await step('each message may pick another; the conversation remembers its latest, and a new chat starts from the chosen model', async () => {
      await page.selectOption('#model-picker', 'p1/gpt-y');
      await ask(page, 'second question');
      await waitIdle(page);
      let asks = await calls(page, 'ask');
      assert.equal(asks[asks.length - 1].model, 'p1/gpt-y');
      assert.equal(await page.evaluate(() => window.__db.convs[0].model), 'p1/gpt-y', 'the conversation stored it');
      // Another chat, with another model, then back to the first: the picker follows each conversation.
      await page.keyboard.press('Control+n');
      await page.selectOption('#model-picker', 'p1/gpt-x');
      await ask(page, 'a different chat');
      await waitIdle(page);
      await page.keyboard.press('Control+h');
      await page.waitForSelector('#history .conv');
      await page.locator('#history .conv').filter({ hasText: 'first question' }).locator('.open').click();
      await page.waitForFunction(() => document.querySelectorAll('.msg.user').length === 2);
      assert.equal(await page.inputValue('#model-picker'), 'p1/gpt-y', 'the first chat shows its own latest model');
      await ask(page, 'back in the first');
      await waitIdle(page);
      asks = await calls(page, 'ask');
      assert.equal(asks[asks.length - 1].model, 'p1/gpt-y', 'and sends with it');
      await page.keyboard.press('Control+n');
      assert.equal(await page.inputValue('#model-picker'), 'p1/gpt-x', 'a new chat uses the model chosen last for new chats');
    });
    await page.context().close();
    page = await open(browser, { models: [models[0]] });
    await step('one approved model: no picker, just the label once there is a thread', async () => {
      assert.equal(await page.isVisible('#foot'), true, 'the footer is always shown');
      await ask(page, 'hello');
      await waitIdle(page);
      await page.waitForSelector('#model-label:not([hidden])');
      assert.equal(await page.isHidden('#model-picker'), true);
    });
    await page.context().close();
  }

  // ------------------------------------------------- server states
  const STATES = {
    offline: { kind: 'offline', message: 'Server offline. Could not reach http://localhost:8080. Check that it is running and that you are on the right network.' },
    rejected: { kind: 'rejected', message: 'Token rejected. The server no longer accepts this app; it may have been revoked.' },
    update_needed: { kind: 'update_needed', message: 'Update needed. This version of Askbar is too old for the server.' },
    model_missing: { kind: 'model_missing', message: 'The model you picked is not approved any more. Pick another.', models: [{ id: 'p1/m', label: 'm via alpha' }, { id: 'p2/m', label: 'm via beta' }] },
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
    if (kind !== 'model_missing') {
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

  page = await open(browser, { status: STATES.model_missing });
  await step('model_missing: picking another model fixes it and updates the label', async () => {
    await page.waitForSelector('#status:not([hidden])');
    const labels = await page.locator('#status-actions [data-action="model"]').allInnerTexts();
    assert.deepEqual(labels, ['Use m via alpha', 'Use m via beta']);
    await page.click('#status-actions [data-action="model"] >> text=Use m via beta');
    await page.waitForSelector('#status', { state: 'hidden' });
    assert.deepEqual(await calls(page, 'set_active_model'), [{ id: 'p2/m' }]);
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

  // ------------------------------------------- privacy notice and AI warning (ADR 0012)
  const policy = (over = {}) => ({
    notice: Object.assign({ enabled: true, text: 'We keep <b>what you ask</b>.\nOnly the administrator can open it.', retention_days: 30, version: 'v1' }, over.notice || {}),
    ai_warning: Object.assign({ enabled: true, text: 'AI can make mistakes. Check important information.' }, over.ai_warning || {}),
  });
  await step('the AI warning sits above the input box as text, and is hidden when switched off or when the server says nothing', async () => {
    const p = await open(browser, { policy: policy({ ai_warning: { text: '<b>AI</b> can err & so can you' }, notice: { enabled: false } }) });
    await p.waitForSelector('#ai-warning:not([hidden])');
    assert.equal(await p.innerText('#ai-warning'), '<b>AI</b> can err & so can you');
    assert.equal(await p.locator('#ai-warning b').count(), 0, 'text, never markup');
    const [w, input] = await Promise.all([p.locator('#ai-warning').boundingBox(), p.locator('#input').boundingBox()]);
    assert.ok(w.y + w.height <= input.y, 'above the input box');
    assert.equal(await p.isHidden('#privacy'), true, 'the notice is off');
    await shot(p, '12-ai-warning');
    const off = await open(browser, { policy: policy({ ai_warning: { enabled: false }, notice: { enabled: false } }) });
    assert.equal(await off.isHidden('#ai-warning'), true);
    const none = await open(browser, {});
    assert.equal(await none.isHidden('#ai-warning'), true);
    assert.equal(await none.isHidden('#privacy'), true, 'an older server says nothing, so nothing is shown');
    await p.context().close(); await off.context().close(); await none.context().close();
  });
  await step('the privacy notice shows its words as text and the retention, once, and again when the version changes', async () => {
    const p = await open(browser, { policy: policy() });
    await p.waitForSelector('#privacy:not([hidden])');
    assert.equal(await p.innerText('#privacy-text'), 'We keep <b>what you ask</b>.\nOnly the administrator can open it.');
    assert.equal(await p.locator('#privacy b').count(), 0, 'text, never markup');
    assert.equal(await p.innerText('#privacy-days'), 'Audit records are kept for 30 days.');
    await shot(p, '13-privacy-notice');
    await p.click('#privacy-ok');
    assert.equal(await p.isHidden('#privacy'), true);
    // The app only remembers the version it showed: it sends nothing back (there is no command for it).
    assert.deepEqual(await p.evaluate(() => window.__calls.map((c) => c[0]).filter((c) => /ack|consent|notice/i.test(c))), []);
    // Shown again when the window opens with the same version: no.
    await p.evaluate(() => window.__listeners['askbar:shown']());
    await p.waitForFunction(() => window.__calls.filter((c) => c[0] === 'bootstrap').length >= 2);
    assert.equal(await p.isHidden('#privacy'), true, 'the same version is not shown twice');
    // And after a reload (the version was kept).
    await p.reload();
    await p.waitForFunction(() => window.__calls.some((c) => c[0] === 'list_conversations'));
    assert.equal(await p.isHidden('#privacy'), true, 'it was kept across a restart');
    // The administrator changes the retention: a new version, and the new number.
    await p.evaluate((np) => { window.__opts.policy = np; }, policy({ notice: { version: 'v2', retention_days: 1 } }));
    await p.evaluate(() => window.__listeners['askbar:shown']());
    await p.waitForSelector('#privacy:not([hidden])');
    assert.equal(await p.innerText('#privacy-days'), 'Audit records are kept for 1 day.');
    // A response that says nothing leaves what is shown as it is.
    await p.evaluate(() => { window.__opts.policy = null; });
    await p.evaluate(() => window.__listeners['askbar:shown']());
    await p.waitForFunction(() => window.__calls.filter((c) => c[0] === 'bootstrap').length >= 3);
    assert.equal(await p.isVisible('#privacy'), true);
    // Switched off by the administrator: gone, whatever the version.
    await p.evaluate((np) => { window.__opts.policy = np; }, policy({ notice: { enabled: false, version: 'v3' } }));
    await p.evaluate(() => window.__listeners['askbar:shown']());
    await p.waitForSelector('#privacy', { state: 'hidden' });
    await p.context().close();
  });
  await step('pairing shows the notice again, even for a version this app showed before', async () => {
    const p = await open(browser, { policy: policy() });
    await p.waitForSelector('#privacy:not([hidden])');
    await p.click('#privacy-ok');
    assert.equal(await p.isHidden('#privacy'), true);
    await p.evaluate(() => window.__listeners['askbar:pair']());
    await p.fill('#pair-input', 'askbar-pair:abc');
    await p.press('#pair-input', 'Enter');
    await p.waitForSelector('#pair', { state: 'hidden' });
    await p.waitForSelector('#privacy:not([hidden])');
    assert.equal(await p.innerText('#privacy-days'), 'Audit records are kept for 30 days.');
    await p.context().close();
  });


  // ----------------------------------------------- hostile replies (ADR 0016, plan step 5.2)
  await step('a hostile reply makes no network request and runs no script, and shows only allowlisted markup (every payload)', async () => {
    const payloads = require('../../../tests/payloads/lib.js');
    const ctx = await browser.newContext({ viewport: { width: 900, height: 760 } });
    await ctx.addInitScript({ path: path.join(ROOT, 'tests/mock-tauri.js') });
    const p = await ctx.newPage();
    const requests = [];
    const errors = [];
    p.on('request', (r) => { if (!r.url().startsWith('file://')) requests.push(r.url()); });
    p.on('pageerror', (e) => errors.push(e.message));
    p.on('dialog', (d) => { errors.push('a dialog opened: ' + d.message()); d.dismiss(); });
    await p.goto('file://' + path.join(ROOT, 'ui/index.html'));
    await p.waitForFunction(() => window.__calls.some((c) => c[0] === 'list_conversations'));
    const rules = payloads.ATTRS;
    const tags = [...payloads.TAGS];
    const failures = [];
    for (const c of payloads.cases()) {
      if (c.input.length > 100000) continue; // the size cases are the renderer's, and are in markdown.test.js
      await p.evaluate((r) => { window.__nextReply = r; window.__pwned = undefined; }, c.input);
      await p.press('#input', 'Control+n');
      await ask(p, 'go');
      await waitIdle(p);
      const bad = await p.evaluate(({ rules, tags }) => {
        const out = [];
        const body = [...document.querySelectorAll('.msg.assistant .body')].pop();
        for (const e of body.querySelectorAll('*')) {
          const name = e.tagName.toLowerCase();
          if (!tags.includes(name)) out.push('element <' + name + '>');
          for (const a of e.attributes) {
            const pattern = (rules[name] || {})[a.name];
            if (!pattern || !new RegExp(pattern).test(a.value)) out.push('<' + name + ' ' + a.name + '="' + a.value.slice(0, 40) + '">');
          }
        }
        if (window.__pwned !== undefined) out.push('a script ran');
        return out;
      }, { rules, tags });
      if (bad.length) failures.push(c.name + ': ' + bad.join(', '));
    }
    assert.deepEqual(failures, []);
    assert.deepEqual(requests, [], 'a request left the page');
    assert.deepEqual(errors, []);
    await ctx.close();
  });

  await browser.close();
  console.log(failures ? `\n${failures} step(s) failed` : '\nall UI steps passed');
  process.exit(failures ? 1 : 0);
})();
