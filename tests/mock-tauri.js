// Injected before the UI loads (Playwright addInitScript). Stands in for the
// Tauri bridge with an in-memory history, so UI behaviour can be tested in a
// plain browser.
(() => {
  const db = { convs: [], nextConv: 1, nextMsg: 1, clock: Math.floor(Date.now() / 1000) };
  const calls = [];
  const listeners = {};
  let cancelFlag = false;
  window.__calls = calls;
  window.__listeners = listeners;
  window.__db = db;
  window.__opts = window.__opts || {};

  class Channel {
    constructor() { this.onmessage = null; }
  }

  const REPLY =
    'Sure. Use **format strings**:\n\n```python\nname = "Ada"\nprint(f"Hello, {name}!")\n```\n\n' +
    '| Option | Meaning |\n|---|---|\n| `-v` | verbose |\n\nSee [the docs](https://example.com/docs).';

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const title = (t) => (t.length > 60 ? t.slice(0, 59) + '…' : t);

  const handlers = {
    bootstrap: () => ({
      ui: {
        theme: window.__opts.theme || 'auto', width: 720, max_height: 640, history_width: 260,
        top_percent: 0.22, hide_on_blur: true, macos_hide_dock_icon: true,
      },
      hotkey: 'Ctrl+Shift+Space', provider: 'mock', model: 'mock-model',
      notice: window.__opts.notice || null,
    }),
    list_conversations: () =>
      db.convs.slice().sort((a, b) => b.updated_at - a.updated_at || b.id - a.id)
        .map((c) => ({ id: c.id, title: c.title, updated_at: c.updated_at, message_count: c.messages.length })),
    load_conversation: ({ id }) => db.convs.find((c) => c.id === id).messages,
    delete_conversation: ({ id }) => { db.convs = db.convs.filter((c) => c.id !== id); },
    cancel_ask: () => { cancelFlag = true; },
    resize_window: () => {},
    hide_window: () => {},
    open_config: () => {},
    open_url: () => {},
    ask: async ({ conversationId, text, onEvent }) => {
      cancelFlag = false;
      let conv = db.convs.find((c) => c.id === conversationId);
      if (!conv) {
        conv = { id: db.nextConv++, title: title(text), updated_at: ++db.clock, messages: [] };
        db.convs.push(conv);
      }
      conv.messages.push({ id: db.nextMsg++, role: 'user', content: text, created_at: db.clock });
      onEvent.onmessage({ event: 'started', conversationId: conv.id });

      if (/fail/i.test(text)) {
        onEvent.onmessage({ event: 'error', message: 'anthropic returned 401: invalid x-api-key. Check api_key in config.yaml.' });
        return;
      }
      const slow = /slow/i.test(text);
      const full = slow ? 'This reply keeps streaming until you stop it. '.repeat(30) : REPLY;
      let sent = '';
      for (let i = 0; i < full.length; i += slow ? 5 : 8) {
        if (cancelFlag) break;
        const piece = full.slice(i, i + (slow ? 5 : 8));
        sent += piece;
        onEvent.onmessage({ event: 'delta', text: piece });
        await sleep(slow ? 40 : 2);
      }
      if (sent) conv.messages.push({ id: db.nextMsg++, role: 'assistant', content: sent, created_at: db.clock });
      conv.updated_at = ++db.clock;
      onEvent.onmessage({ event: cancelFlag ? 'cancelled' : 'done' });
    },
  };

  window.__TAURI__ = {
    core: {
      Channel,
      invoke: async (cmd, args = {}) => {
        calls.push([cmd, JSON.parse(JSON.stringify(args, (k, v) => (k === 'onEvent' ? '[channel]' : v)))]);
        const h = handlers[cmd];
        if (!h) throw new Error('unknown command ' + cmd);
        return h(args);
      },
    },
    event: { listen: async (name, cb) => { listeners[name] = cb; return () => {}; } },
  };
})();
