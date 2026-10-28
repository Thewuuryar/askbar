// Askbar UI. Talks to the Rust side only through the commands in main.rs.
(() => {
  'use strict';

  const { invoke, Channel } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  const PAD = 32; // keep in sync with --pad in style.css

  const $ = (id) => document.getElementById(id);
  const el = {
    card: $('card'),
    input: $('input'),
    thread: $('thread'),
    foot: $('foot'),
    stop: $('stop'),
    histToggle: $('history-toggle'),
    history: $('history'),
    histList: $('history-list'),
    newChat: $('new-chat'),
    notice: $('notice'),
    noticeText: $('notice-text'),
    noticeConfig: $('notice-config'),
    noticeClose: $('notice-close'),
    status: $('status'),
    statusText: $('status-text'),
    statusActions: $('status-actions'),
    modelLabel: $('model-label'),
  };

  const state = {
    ui: null,
    convId: null,
    messages: [],        // { role, content, node, bodyNode }
    streaming: false,
    historyOpen: false,
    conversations: [],
    armed: null,         // { id, timer } for two-step delete
    pinned: true,        // thread follows new output until the user scrolls up
  };

  // ------------------------------------------------------------ helpers

  function copyViaTextarea(text) {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.cssText = 'position:fixed;opacity:0;pointer-events:none';
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand('copy'); } catch (_) { /* nothing else to try */ }
    ta.remove();
    el.input.focus();
  }

  async function copyText(text, button) {
    try {
      await navigator.clipboard.writeText(text);
    } catch (_) {
      copyViaTextarea(text);
    }
    if (button) {
      const before = button.textContent;
      button.textContent = 'Copied';
      setTimeout(() => { button.textContent = before; }, 1200);
    }
  }

  function autosize() {
    el.input.style.height = 'auto';
    el.input.style.height = Math.min(el.input.scrollHeight, 180) + 'px';
  }

  // The window is sized to the card plus the transparent shadow margin.
  let lastSize = { w: 0, h: 0 };
  function layout() {
    const w = Math.ceil(el.card.offsetWidth) + PAD * 2;
    const h = Math.ceil(el.card.offsetHeight) + PAD * 2;
    if (w !== lastSize.w || h !== lastSize.h) {
      lastSize = { w, h };
      invoke('resize_window', { width: w, height: h }).catch(() => {});
    }
  }

  function applyUi(ui) {
    const root = document.documentElement;
    root.dataset.theme = ui.theme;
    root.style.setProperty('--card-w', ui.width + 'px');
    root.style.setProperty('--hist-w', ui.history_width + 'px');
    root.style.setProperty('--card-max-h', ui.max_height + 'px');
  }

  function syncChrome() {
    const hasThread = state.messages.length > 0;
    el.thread.hidden = !hasThread;
    el.foot.hidden = !hasThread;
    el.card.dataset.thread = hasThread ? 'open' : 'empty';
    el.card.dataset.history = state.historyOpen ? 'open' : 'closed';
    el.history.hidden = !state.historyOpen;
    el.histToggle.setAttribute('aria-pressed', String(state.historyOpen));
    el.stop.hidden = !state.streaming;
    layout();
  }

  // -------------------------------------------------------------- notice

  function showNotice(text) {
    el.noticeText.textContent = text;
    el.noticeConfig.hidden = !/config/i.test(text);
    el.notice.hidden = false;
    layout();
  }
  function hideNotice() {
    if (!el.notice.hidden) { el.notice.hidden = true; layout(); }
  }

  // -------------------------------------------------------------- status
  // The backend reports problems with the server (offline, token rejected,
  // update needed, provider gone). `null` means all is well.

  function actionButton(label, onClick, name) {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'text-btn';
    b.dataset.action = name;
    b.textContent = label;
    b.addEventListener('click', onClick);
    return b;
  }

  function showStatus(st) {
    el.statusActions.textContent = '';
    if (!st) {
      if (!el.status.hidden) { el.status.hidden = true; layout(); }
      return;
    }
    el.status.dataset.kind = st.kind;
    el.statusText.textContent = st.message;
    for (const name of st.providers || []) {
      el.statusActions.appendChild(actionButton('Use ' + name, async () => {
        try { await invoke('set_active_provider', { name }); } catch (e) { showNotice(String(e)); }
        await refreshBootstrap().catch(() => {});
        await checkStatus();
      }, 'provider'));
    }
    el.statusActions.appendChild(actionButton('Retry', async () => {
      await refreshBootstrap().catch(() => {});
      await checkStatus();
    }, 'retry'));
    el.status.hidden = false;
    layout();
  }

  async function checkStatus() {
    let st = null;
    try { st = await invoke('server_status'); } catch (_) { /* local mode has nothing to report */ }
    showStatus(st);
    return st;
  }

  // ------------------------------------------------------------ messages

  function render(msg) {
    msg.bodyNode.innerHTML = msg.role === 'assistant' ? window.renderMarkdown(msg.content) : '';
    if (msg.role === 'user') msg.bodyNode.textContent = msg.content;
    if (state.pinned) el.thread.scrollTop = el.thread.scrollHeight;
  }

  let renderQueued = new Set();
  function scheduleRender(msg) {
    if (renderQueued.has(msg)) return;
    renderQueued.add(msg);
    requestAnimationFrame(() => { renderQueued.delete(msg); render(msg); });
  }

  function addMessage(role, content, opts = {}) {
    const node = document.createElement('div');
    node.className = 'msg ' + role + (opts.streaming ? ' streaming' : '');
    const actions = document.createElement('div');
    actions.className = 'actions';
    const copy = document.createElement('button');
    copy.type = 'button';
    copy.className = 'copy-msg';
    copy.textContent = 'Copy';
    copy.setAttribute('aria-label', 'Copy this message');
    actions.appendChild(copy);
    const bodyNode = document.createElement('div');
    bodyNode.className = 'body';
    node.append(bodyNode, actions);
    el.thread.appendChild(node);

    const msg = { role, content, node, bodyNode };
    node._msg = msg;
    state.messages.push(msg);
    render(msg);
    syncChrome();
    return msg;
  }

  function addError(message) {
    const node = document.createElement('div');
    node.className = 'msg error';
    const body = document.createElement('div');
    body.className = 'body';
    body.textContent = message;
    node.appendChild(body);
    if (/config\.yaml|API key/i.test(message)) {
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'text-btn open-config';
      btn.textContent = 'Open config file';
      node.appendChild(btn);
    }
    el.thread.appendChild(node);
    if (state.pinned) el.thread.scrollTop = el.thread.scrollHeight;
    el.thread.hidden = false;
    el.foot.hidden = false;
    el.card.dataset.thread = 'open';
    layout();
  }

  function clearThread() {
    state.messages = [];
    el.thread.textContent = '';
    state.pinned = true;
    syncChrome();
  }

  function removeMessage(msg) {
    msg.node.remove();
    state.messages = state.messages.filter((m) => m !== msg);
  }

  // ---------------------------------------------------------------- send

  function setStreaming(on) {
    state.streaming = on;
    syncChrome();
  }

  async function send() {
    const text = el.input.value.trim();
    if (!text || state.streaming) return;
    el.input.value = '';
    autosize();
    hideNotice();
    state.pinned = true;

    addMessage('user', text);
    const reply = addMessage('assistant', '', { streaming: true });
    setStreaming(true);

    const channel = new Channel();
    await new Promise((resolve) => {
      let finished = false;
      const finish = () => { if (!finished) { finished = true; resolve(); } };

      channel.onmessage = (m) => {
        switch (m.event) {
          case 'started':
            state.convId = m.conversationId;
            break;
          case 'delta':
            reply.content += m.text;
            scheduleRender(reply);
            break;
          case 'done':
            showStatus(null);
            finish();
            break;
          case 'cancelled':
            finish();
            break;
          case 'error':
            if (!reply.content.trim()) removeMessage(reply);
            addError(m.message);
            checkStatus();
            finish();
            break;
        }
      };

      invoke('ask', { conversationId: state.convId, text, onEvent: channel })
        .then(() => setTimeout(finish, 1500)) // terminal event normally arrives first
        .catch((e) => {
          if (!reply.content.trim()) removeMessage(reply);
          addError(String(e));
          checkStatus();
          finish();
        });
    });

    reply.node.classList.remove('streaming');
    render(reply);
    setStreaming(false);
    refreshHistory();
    el.input.focus();
  }

  function cancel() {
    if (state.streaming) invoke('cancel_ask').catch(() => {});
  }

  // ------------------------------------------------------------- history

  const DAY = 86400;
  function bucket(updatedAt) {
    const now = new Date();
    const startToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000;
    if (updatedAt >= startToday) return 'Today';
    if (updatedAt >= startToday - DAY) return 'Yesterday';
    if (updatedAt >= startToday - 6 * DAY) return 'Previous 7 days';
    return 'Older';
  }

  function renderHistory() {
    el.histList.textContent = '';
    if (!state.conversations.length) {
      const p = document.createElement('p');
      p.className = 'history-empty';
      p.textContent = 'No saved conversations yet. Ask something and it will appear here.';
      el.histList.appendChild(p);
      return;
    }
    let group = null;
    for (const c of state.conversations) {
      const b = bucket(c.updated_at);
      if (b !== group) {
        group = b;
        const h = document.createElement('div');
        h.className = 'history-group';
        h.textContent = b;
        el.histList.appendChild(h);
      }
      const row = document.createElement('div');
      row.className = 'conv' + (c.id === state.convId ? ' active' : '');
      row.dataset.id = String(c.id);

      const open = document.createElement('button');
      open.type = 'button';
      open.className = 'open';
      open.textContent = c.title;
      open.title = c.title;

      const del = document.createElement('button');
      del.type = 'button';
      del.className = 'del';
      del.textContent = state.armed && state.armed.id === c.id ? 'Delete?' : 'Delete';
      if (state.armed && state.armed.id === c.id) del.classList.add('armed');
      del.setAttribute('aria-label', 'Delete conversation: ' + c.title);

      row.append(open, del);
      el.histList.appendChild(row);
    }
  }

  async function refreshHistory() {
    try {
      state.conversations = await invoke('list_conversations');
    } catch (e) {
      state.conversations = [];
    }
    renderHistory();
  }

  function disarm() {
    if (state.armed) { clearTimeout(state.armed.timer); state.armed = null; }
  }

  async function onDelete(id) {
    if (!state.armed || state.armed.id !== id) {
      disarm();
      state.armed = { id, timer: setTimeout(() => { state.armed = null; renderHistory(); }, 3000) };
      renderHistory();
      return;
    }
    disarm();
    try {
      await invoke('delete_conversation', { id });
    } catch (e) {
      showNotice(String(e));
    }
    if (id === state.convId) newChat();
    await refreshHistory();
  }

  async function openConversation(id) {
    if (state.streaming) return;
    let rows;
    try {
      rows = await invoke('load_conversation', { id });
    } catch (e) {
      showNotice(String(e));
      return;
    }
    hideNotice();
    clearThread();
    state.convId = id;
    for (const r of rows) addMessage(r.role, r.content);
    state.pinned = true;
    el.thread.scrollTop = el.thread.scrollHeight;
    renderHistory();
    el.input.focus();
  }

  function newChat() {
    if (state.streaming) return;
    state.convId = null;
    clearThread();
    hideNotice();
    renderHistory();
    el.input.focus();
  }

  function toggleHistory(force) {
    state.historyOpen = typeof force === 'boolean' ? force : !state.historyOpen;
    syncChrome();
    if (state.historyOpen) refreshHistory();
    el.input.focus();
  }

  // -------------------------------------------------------------- events

  el.input.addEventListener('input', autosize);
  el.input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  });

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      if (state.streaming) cancel(); else invoke('hide_window');
      return;
    }
    if (e.ctrlKey && !e.metaKey && !e.altKey) {
      const k = e.key.toLowerCase();
      if (k === 'h' && !e.shiftKey) { e.preventDefault(); toggleHistory(); }
      else if (k === 'n' && !e.shiftKey) { e.preventDefault(); newChat(); }
      else if (k === 'c' && e.shiftKey) {
        e.preventDefault();
        const last = [...state.messages].reverse().find((m) => m.role === 'assistant' && m.content);
        if (last) copyText(last.content, last.node.querySelector('.copy-msg'));
      }
    }
  });

  el.thread.addEventListener('scroll', () => {
    const t = el.thread;
    state.pinned = t.scrollHeight - t.scrollTop - t.clientHeight < 40;
  });

  el.thread.addEventListener('click', (e) => {
    const t = e.target;
    if (!(t instanceof Element)) return;

    const copyMsg = t.closest('.copy-msg');
    if (copyMsg) {
      const msg = copyMsg.closest('.msg')._msg;
      if (msg) copyText(msg.content, copyMsg);
      return;
    }
    const copyCode = t.closest('.copy-code');
    if (copyCode) {
      const code = copyCode.closest('.code').querySelector('pre code');
      copyText(code.textContent, copyCode);
      return;
    }
    if (t.closest('.open-config')) { invoke('open_config').catch((err) => showNotice(String(err))); return; }

    const link = t.closest('a[href]');
    if (link) {
      e.preventDefault();
      invoke('open_url', { url: link.getAttribute('href') }).catch(() => {});
    }
  });

  el.histList.addEventListener('click', (e) => {
    const t = e.target;
    if (!(t instanceof Element)) return;
    const row = t.closest('.conv');
    if (!row) return;
    const id = Number(row.dataset.id);
    if (t.closest('.del')) onDelete(id);
    else if (t.closest('.open')) openConversation(id);
  });

  el.histToggle.addEventListener('click', () => toggleHistory());
  el.newChat.addEventListener('click', newChat);
  el.stop.addEventListener('click', cancel);
  el.noticeClose.addEventListener('click', hideNotice);
  el.noticeConfig.addEventListener('click', () => invoke('open_config').catch(() => {}));

  new ResizeObserver(layout).observe(el.card);

  // ---------------------------------------------------------------- boot

  async function refreshBootstrap() {
    const b = await invoke('bootstrap');
    state.ui = b.ui;
    applyUi(b.ui);
    el.modelLabel.textContent = b.model ? b.model + ' via ' + b.provider : '';
    if (b.notice) showNotice(b.notice);
  }

  async function onShown() {
    try { await refreshBootstrap(); } catch (_) { /* keep the previous settings */ }
    checkStatus();
    if (state.historyOpen) refreshHistory();
    el.input.focus();
    el.input.select();
    layout();
  }

  listen('askbar:shown', onShown);

  (async () => {
    try { await refreshBootstrap(); } catch (e) { showNotice(String(e)); }
    checkStatus();
    await refreshHistory();
    syncChrome();
    el.input.focus();
  })();
})();
