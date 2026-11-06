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
    pair: $('pair'),
    pairInput: $('pair-input'),
    pairSubmit: $('pair-submit'),
    pairCancel: $('pair-cancel'),
    pairError: $('pair-error'),
    pairOk: $('pair-ok'),
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
    for (const p of st.providers || []) {
      el.statusActions.appendChild(actionButton('Use ' + p.name, async () => {
        try { await invoke('set_active_provider', { id: p.id }); } catch (e) { showNotice(String(e)); }
        await refreshBootstrap().catch(() => {});
        await checkStatus();
      }, 'provider'));
    }
    if (st.kind === 'rejected' || st.kind === 'not_paired') {
      el.statusActions.appendChild(actionButton('Pair this app', () => openPairing(), 'pair'));
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

  // ------------------------------------------------------------- pairing

  const PAIR_TIMEOUT_MS = 15000;

  function setPairBusy(busy) {
    el.pairSubmit.disabled = busy;
    el.pairInput.disabled = busy;
    el.pairSubmit.textContent = busy ? 'Pairing…' : 'Pair';
  }

  function openPairing() {
    el.pair.dataset.state = '';
    el.pairError.hidden = true;
    el.pairOk.hidden = true;
    el.pairInput.value = '';
    setPairBusy(false);
    el.pair.hidden = false;
    layout();
    el.pairInput.focus();
  }

  function closePairing() {
    if (el.pair.hidden) return false;
    el.pair.hidden = true;
    layout();
    el.input.focus();
    return true;
  }

  function serverName(url) {
    try { return new URL(url).host; } catch (_) { return String(url || 'the server'); }
  }

  async function submitPairing(e) {
    e.preventDefault();
    const pairing = el.pairInput.value.trim();
    if (!pairing || el.pairInput.disabled) return;
    el.pairError.hidden = true;
    setPairBusy(true);
    let server;
    try {
      let timer;
      const timeout = new Promise((_, reject) => {
        timer = setTimeout(() => reject('The app did not answer. Try again.'), PAIR_TIMEOUT_MS);
      });
      try { server = await Promise.race([invoke('pair', { pairing }), timeout]); }
      finally { clearTimeout(timer); }
    } catch (err) {
      el.pairError.textContent = String(err);
      el.pairError.hidden = false;
      setPairBusy(false);
      el.pairInput.select();
      el.pairInput.focus();
      layout();
      return;
    }
    // Say it worked, then get out of the way.
    el.pairOk.textContent = 'Paired with ' + serverName(server) + '.';
    el.pairOk.hidden = false;
    el.pair.dataset.state = 'done';
    layout();
    await new Promise((r) => setTimeout(r, 1000));
    closePairing();
    newChat();
    await refreshBootstrap().catch(() => {});
    await checkStatus();
    await refreshHistory();
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

  function addError(message, retry) {
    const node = document.createElement('div');
    node.className = 'msg error';
    const body = document.createElement('div');
    body.className = 'body';
    body.textContent = message;
    node.appendChild(body);
    if (retry) {
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'text-btn retry-reply';
      btn.textContent = 'Try again';
      btn.addEventListener('click', () => {
        if (state.streaming) return;
        node.remove();
        retry();
      });
      node.appendChild(btn);
    }
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
    clearWaiting();
    clearUnsent();
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

  function showServedBy(reply, name) {
    let tag = reply.node.querySelector('.served-by');
    if (!tag) {
      tag = document.createElement('div');
      tag.className = 'served-by';
      reply.node.insertBefore(tag, reply.node.querySelector('.actions'));
    }
    tag.textContent = 'Answered by ' + name;
  }

  // Shows one reply as it streams in. `start(channel)` begins the stream (a new send, or joining
  // one that is already running on the server) and returns the command's promise.
  async function streamInto(reply, start, userMsg) {
    setStreaming(true);
    const channel = new Channel();
    let accepted = false; // the server has taken the message once its reply has begun
    await new Promise((resolve) => {
      let finished = false;
      const finish = () => { if (!finished) { finished = true; resolve(); } };

      channel.onmessage = (m) => {
        switch (m.event) {
          case 'started':
            accepted = true;
            state.convId = m.conversationId;
            break;
          case 'provider':
            // Only worth saying when the server used another provider than the one chosen.
            if (m.name && m.name !== state.provider) showServedBy(reply, m.name);
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
            // A reply that ran out of time keeps what it wrote; trying again asks the same question
            // as a new reply, below this one.
            addError(m.message, m.kind === 'timed_out' && userMsg ? () => retryAsked(userMsg.content) : undefined);
            checkStatus();
            finish();
            break;
        }
      };

      start(channel)
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
    // A message the server never took is not part of the conversation: it moves to the unsent list.
    if (!accepted && userMsg) removeMessage(userMsg);
    refreshHistory();
    el.input.focus();
    syncUnsent();
    syncPending(); // messages that were waiting behind this reply may be running now
  }

  async function retryAsked(text) {
    state.pinned = true;
    const reply = addMessage('assistant', '', { streaming: true });
    await streamInto(reply, (channel) => invoke('ask', { conversationId: state.convId, text, onEvent: channel }), null);
  }

  async function send() {
    const text = el.input.value.trim();
    if (!text || state.streaming) return;
    el.input.value = '';
    autosize();
    hideNotice();
    state.pinned = true;

    const userMsg = addMessage('user', text);
    const reply = addMessage('assistant', '', { streaming: true });
    await streamInto(reply, (channel) => invoke('ask', { conversationId: state.convId, text, onEvent: channel }), userMsg);
  }

  // ------------------------------------------------------- messages not sent

  let unsentRows = [];

  function clearUnsent() {
    for (const r of unsentRows) r.remove();
    unsentRows = [];
  }

  function addUnsent(u) {
    const node = document.createElement('div');
    node.className = 'msg user unsent';
    node.dataset.request = u.clientRequestId;
    const body = document.createElement('div');
    body.className = 'body';
    body.textContent = u.text;
    const note = document.createElement('div');
    note.className = 'wait-note';
    note.textContent = 'Not sent';
    const mk = (cls, label, aria) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'text-btn ' + cls;
      b.textContent = label;
      b.setAttribute('aria-label', aria);
      return b;
    };
    const send = mk('unsent-send', 'Send', 'Send this message');
    const edit = mk('unsent-edit', 'Edit', 'Edit this message before sending');
    const discard = mk('unsent-discard', 'Discard', 'Discard this message');
    const gone = () => { node.remove(); unsentRows = unsentRows.filter((r) => r !== node); };
    send.addEventListener('click', async () => {
      if (state.streaming) return;
      gone();
      hideNotice();
      state.pinned = true;
      const userMsg = addMessage('user', u.text);
      const reply = addMessage('assistant', '', { streaming: true });
      await streamInto(reply, (channel) => invoke('resend', { clientRequestId: u.clientRequestId, onEvent: channel }), userMsg);
    });
    edit.addEventListener('click', async () => {
      try { await invoke('discard_unsent', { clientRequestId: u.clientRequestId }); } catch (e) { showNotice(String(e)); return; }
      gone();
      el.input.value = u.text;
      autosize();
      el.input.focus();
    });
    discard.addEventListener('click', async () => {
      try { await invoke('discard_unsent', { clientRequestId: u.clientRequestId }); } catch (e) { showNotice(String(e)); return; }
      gone();
      layout();
    });
    node.append(body, note, send, edit, discard);
    el.thread.appendChild(node);
    unsentRows.push(node);
  }

  // Lists the open conversation's messages that never reached the server. Nothing is sent by
  // itself: each one waits for Send, Edit or Discard.
  async function syncUnsent() {
    let list;
    try { list = await invoke('unsent_messages'); } catch (_) { return; }
    clearUnsent();
    const mine = list.filter((u) => (u.conversationId || null) === state.convId);
    for (const u of mine) addUnsent(u);
    if (mine.length) {
      el.thread.hidden = false;
      el.foot.hidden = false;
      el.card.dataset.thread = 'open';
      if (state.pinned) el.thread.scrollTop = el.thread.scrollHeight;
    }
    layout();
  }

  // ----------------------------------------------- replies still going on the server

  let waiting = [];       // rows for messages queued behind a running reply
  let pendingTimer = null;

  function clearWaiting() {
    for (const w of waiting) w.remove();
    waiting = [];
    clearTimeout(pendingTimer);
    pendingTimer = null;
  }

  function addWaiting(p) {
    const node = document.createElement('div');
    node.className = 'msg user waiting';
    node.dataset.generation = p.generationId;
    node.style.opacity = '0.6';
    const body = document.createElement('div');
    body.className = 'body';
    body.textContent = p.prompt;
    const note = document.createElement('div');
    note.className = 'wait-note';
    note.textContent = 'Waiting for the reply above to finish';
    const del = document.createElement('button');
    del.type = 'button';
    del.className = 'text-btn del-queued';
    del.textContent = 'Remove';
    del.setAttribute('aria-label', 'Remove this waiting message');
    del.addEventListener('click', async () => {
      del.disabled = true;
      try { await invoke('cancel_generation', { id: p.generationId }); } catch (e) { showNotice(String(e)); }
      node.remove();
      waiting = waiting.filter((w) => w !== node);
      syncPending();
    });
    node.append(body, note, del);
    el.thread.appendChild(node);
    waiting.push(node);
  }

  // Looks at what the server is still doing in the open conversation: joins the reply that is
  // running, and lists the messages waiting behind it with a way to remove each.
  async function syncPending() {
    const id = state.convId;
    if (!id || state.streaming) return;
    let pending;
    try { pending = await invoke('load_pending', { id }); } catch (_) { return; }
    if (id !== state.convId || state.streaming) return;
    clearWaiting();
    const running = pending.find((p) => p.status === 'running');
    if (running) {
      state.pinned = true;
      // A message that waited has just started: its question is stored now, but not shown yet.
      const last = state.messages[state.messages.length - 1];
      if (!last || last.role !== 'user') addMessage('user', running.prompt);
      const reply = addMessage('assistant', '', { streaming: true });
      for (const p of pending) if (p.status === 'queued') addWaiting(p);
      el.thread.hidden = false;
      layout();
      streamInto(reply, (channel) => invoke('attach', { generationId: running.generationId, onEvent: channel }));
      return;
    }
    const queued = pending.filter((p) => p.status === 'queued');
    for (const p of queued) addWaiting(p);
    // A queued message starts a moment after the reply before it ends; look again until it does.
    if (queued.length) pendingTimer = setTimeout(syncPending, 2000);
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
    syncUnsent();
    syncPending();
  }

  function newChat() {
    if (state.streaming) return;
    state.convId = null;
    clearThread();
    hideNotice();
    renderHistory();
    el.input.focus();
    syncUnsent();
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
      if (closePairing()) return;
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
    const id = row.dataset.id;
    if (t.closest('.del')) onDelete(id);
    else if (t.closest('.open')) openConversation(id);
  });

  el.histToggle.addEventListener('click', () => toggleHistory());
  el.newChat.addEventListener('click', newChat);
  el.stop.addEventListener('click', cancel);
  el.noticeClose.addEventListener('click', hideNotice);
  el.pair.addEventListener('submit', submitPairing);
  el.pairCancel.addEventListener('click', closePairing);
  el.noticeConfig.addEventListener('click', () => invoke('open_config').catch(() => {}));

  new ResizeObserver(layout).observe(el.card);

  // ---------------------------------------------------------------- boot

  async function refreshBootstrap() {
    const b = await invoke('bootstrap');
    state.ui = b.ui;
    applyUi(b.ui);
    state.provider = b.provider;
    el.modelLabel.textContent = b.model ? b.model + ' via ' + b.provider : '';
    if (b.notice) showNotice(b.notice);
  }

  async function onShown() {
    try { await refreshBootstrap(); } catch (_) { /* keep the previous settings */ }
    checkStatus();
    if (state.historyOpen) refreshHistory();
    syncUnsent();
    syncPending();
    el.input.focus();
    el.input.select();
    layout();
  }

  listen('askbar:shown', onShown);
  listen('askbar:pair', () => openPairing());

  (async () => {
    try { await refreshBootstrap(); } catch (e) { showNotice(String(e)); }
    checkStatus();
    await refreshHistory();
    syncChrome();
    syncUnsent();
    el.input.focus();
  })();
})();
