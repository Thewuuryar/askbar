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
    share: $('share'),
    shareName: $('share-name'),
    shareSubmit: $('share-submit'),
    shareCancel: $('share-cancel'),
    shareOut: $('share-out'),
    shareKey: $('share-key'),
    shareCopy: $('share-copy'),
    shareError: $('share-error'),
    status: $('status'),
    statusText: $('status-text'),
    statusActions: $('status-actions'),
    modelLabel: $('model-label'),
    modelPicker: $('model-picker'),
    aiWarning: $('ai-warning'),
    privacy: $('privacy'),
    privacyText: $('privacy-text'),
    privacyDays: $('privacy-days'),
    privacyOk: $('privacy-ok'),
    linkConfirm: $('link-confirm'),
    linkUrl: $('link-confirm-url'),
    linkOpen: $('link-open'),
    linkCancel: $('link-cancel'),
  };

  const state = {
    ui: null,
    convId: null,
    models: [],          // the approved models, { id, label }
    model: null,         // the model this chat picked (`provider/model`); null means the one chosen for new chats
    messages: [],        // { role, content, node, bodyNode }
    streaming: false,
    historyOpen: false,
    conversations: [],
    armed: null,         // { id, timer } for two-step delete
    pinned: true,        // thread follows new output until the user scrolls away from it
    newestFirst: false,  // the display setting: newest turn first, under the bar
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
  // Newest first: the latest turn sits just under the bar. A turn is a question and what follows it
  // (its reply, its error line), and keeps that order inside; only the turns are reversed. Done with
  // the flex `order` of each row, so the rows themselves stay in time order.
  function orderTurns() {
    let turn = 0;
    for (const row of el.thread.children) {
      if (row.matches('.msg.user')) turn += 1;
      row.style.order = state.newestFirst ? String(-turn) : '';
    }
  }

  // Keeps the newest output in view: the bottom of the thread, or its top when newest is first.
  function followNewest() {
    el.thread.scrollTop = state.newestFirst ? 0 : el.thread.scrollHeight;
  }

  function layout() {
    orderTurns();
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
    state.newestFirst = !!ui.newest_first;
    el.thread.dataset.order = state.newestFirst ? 'newest' : 'oldest';
    orderTurns();
  }

  function syncChrome() {
    const hasThread = state.messages.length > 0;
    el.thread.hidden = !hasThread;
    el.foot.hidden = false; // always shown: it holds the model and the key hints
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
    for (const m of st.models || []) {
      el.statusActions.appendChild(actionButton('Use ' + m.label, async () => {
        try { await invoke('set_active_model', { id: m.id }); } catch (e) { showNotice(String(e)); }
        state.model = null;
        await refreshBootstrap().catch(() => {});
        await checkStatus();
      }, 'model'));
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

  function openPairing() {
    closeShare();
    el.pairError.hidden = true;
    el.pairInput.value = '';
    el.pair.hidden = false;
    layout();
    el.pairInput.focus();
  }

  // A link in a reply opens only after its whole address has been shown (ADR 0044, D18), and only if it is http or https. The
  // label a reply puts on a link says nothing about where it goes. `open_url` checks the scheme again in the backend.
  let pendingLink = null;

  function openableUrl(u) {
    return typeof u === 'string' && u.length <= 2048 && /^https?:\/\/[^\s\u0000-\u001f\u007f]+$/.test(u);
  }

  function askToOpen(url) {
    if (!openableUrl(url)) return;
    pendingLink = url;
    el.linkUrl.textContent = url;
    el.linkConfirm.hidden = false;
    layout();
    el.linkCancel.focus();
  }

  function closeLinkConfirm() {
    if (!pendingLink) return false;
    pendingLink = null;
    el.linkConfirm.hidden = true;
    el.linkUrl.textContent = '';
    layout();
    el.input.focus();
    return true;
  }

  function openPendingLink() {
    const url = pendingLink;
    closeLinkConfirm();
    if (url) invoke('open_url', { url }).catch(() => {});
  }

  function closePairing() {
    if (el.pair.hidden) return false;
    el.pair.hidden = true;
    layout();
    el.input.focus();
    return true;
  }

  // "Pair another app" (ADR 0008): a one-time key for this user's next app.
  function openShare() {
    closePairing();
    el.shareError.hidden = true;
    el.shareOut.hidden = true;
    el.shareKey.value = '';
    el.shareName.value = '';
    el.share.hidden = false;
    layout();
    el.shareName.focus();
  }

  function closeShare() {
    if (el.share.hidden) return false;
    el.share.hidden = true;
    el.shareKey.value = '';
    layout();
    el.input.focus();
    return true;
  }

  async function submitShare(e) {
    e.preventDefault();
    el.shareSubmit.disabled = true;
    el.shareError.hidden = true;
    try {
      el.shareKey.value = await invoke('new_pairing_key', { name: el.shareName.value.trim() });
      el.shareOut.hidden = false;
      el.shareKey.select();
    } catch (err) {
      el.shareError.textContent = String(err);
      el.shareError.hidden = false;
    }
    el.shareSubmit.disabled = false;
    layout();
  }

  async function submitPairing(e) {
    e.preventDefault();
    const pairing = el.pairInput.value.trim();
    if (!pairing) return;
    el.pairSubmit.disabled = true;
    try {
      await invoke('pair', { pairing });
    } catch (err) {
      el.pairError.textContent = String(err);
      el.pairError.hidden = false;
      el.pairSubmit.disabled = false;
      layout();
      return;
    }
    el.pairSubmit.disabled = false;
    el.pairInput.value = '';
    closePairing();
    newChat();
    // A new pairing is the moment the notice is for: show it even if this app showed the same words to another account.
    forgetVersion();
    await refreshBootstrap().catch(() => {});
    await checkStatus();
    await refreshHistory();
  }

  // ------------------------------------------------------------ messages

  // Trusted Types (ADR 0044, D17). The CSP requires them, so the browser refuses a string for any HTML sink, and allows one policy:
  // `askbar-markdown`, which turns Markdown into HTML with `renderMarkdown` and nothing else. The one `innerHTML` in this app is
  // below; if it were ever handed anything but a policy's output, the browser would refuse it. A webview with no Trusted Types
  // has no such rule, and `renderMarkdown`'s own escaping is what protects it. If the policy is refused, the reply is shown as
  // plain text: readable, and never markup.
  let markdownPolicy = null;
  try {
    if (window.trustedTypes) {
      markdownPolicy = window.trustedTypes.createPolicy('askbar-markdown', { createHTML: (source) => window.renderMarkdown(source) });
    }
  } catch (_) { markdownPolicy = null; }

  function setReply(node, markdown) {
    if (window.trustedTypes && !markdownPolicy) {
      node.textContent = markdown;
      return;
    }
    // The one HTML sink in the app.
    node.innerHTML = markdownPolicy ? markdownPolicy.createHTML(markdown) : window.renderMarkdown(markdown);
  }

  function render(msg) {
    if (msg.role === 'assistant') setReply(msg.bodyNode, msg.content);
    else msg.bodyNode.textContent = msg.content;
    if (state.pinned) followNewest();
  }

  let renderQueued = new Set();
  function scheduleRender(msg) {
    if (renderQueued.has(msg)) return;
    renderQueued.add(msg);
    requestAnimationFrame(() => { renderQueued.delete(msg); render(msg); });
  }

  // A message's date and time, in the person's own locale: `created_at` is unix seconds.
  function formatTime(secs) {
    return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  }

  // The little chip beside a reply's time: which provider and model wrote it (plan step 1.13).
  // Text goes in through setAttribute only; replies stored before servers recorded it get none.
  function setProvider(msg, provider) {
    if (!provider || !provider.name) return;
    msg.provider = provider;
    let chip = msg.node.querySelector('.by');
    if (!chip) {
      chip = document.createElement('span');
      chip.className = 'by';
      chip.tabIndex = 0;
      chip.setAttribute('role', 'img');
      msg.node.querySelector('.meta').insertBefore(chip, msg.node.querySelector('.copy-msg'));
    }
    const label = (provider.model ? 'Model: ' + provider.model + ' · ' : '') + 'Provider: ' + provider.name;
    chip.title = label;
    chip.setAttribute('aria-label', label);
  }

  // A message, with its meta row: the date and time, the provider chip on a reply, and the copy
  // button. The row's space is kept at all times; it only shows on hover or when one of its controls
  // has keyboard focus (they stay reachable by Tab), so nothing moves when it appears.
  function addMessage(role, content, opts = {}) {
    const node = document.createElement('div');
    node.className = 'msg ' + role + (opts.streaming ? ' streaming' : '');
    const meta = document.createElement('div');
    meta.className = 'meta';
    const createdAt = opts.createdAt || Math.floor(Date.now() / 1000);
    const when = document.createElement('time');
    when.className = 'when';
    when.dateTime = new Date(createdAt * 1000).toISOString();
    when.textContent = formatTime(createdAt);
    const copy = document.createElement('button');
    copy.type = 'button';
    copy.className = 'copy-msg';
    copy.title = 'Copy';
    copy.setAttribute('aria-label', 'Copy this message');
    meta.append(when, copy);
    const bodyNode = document.createElement('div');
    bodyNode.className = 'body';
    node.append(bodyNode, meta);
    el.thread.appendChild(node);

    const msg = { role, content, node, bodyNode };
    node._msg = msg;
    state.messages.push(msg);
    if (role === 'assistant') setProvider(msg, opts.provider);
    render(msg);
    syncChrome();
    return msg;
  }

  // An error line in the thread. With `question`, it belongs to that failed question and offers
  // Edit (its text back in the box) and Try again (asks it again, below).
  function addError(message, question) {
    const node = document.createElement('div');
    node.className = 'msg error';
    const body = document.createElement('div');
    body.className = 'body';
    body.textContent = message;
    node.appendChild(body);
    if (question !== undefined) {
      const mk = (cls, label, aria, fn) => {
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'text-btn ' + cls;
        btn.textContent = label;
        btn.setAttribute('aria-label', aria);
        btn.addEventListener('click', fn);
        node.appendChild(btn);
      };
      mk('retry-reply', 'Try again', 'Ask this question again', () => { if (!state.streaming) askText(question); });
      mk('edit-question', 'Edit', 'Edit this question in the box', () => {
        el.input.value = question;
        autosize();
        el.input.focus();
      });
    }
    if (/config\.yaml|API key/i.test(message)) {
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'text-btn open-config';
      btn.textContent = 'Open config file';
      node.appendChild(btn);
    }
    el.thread.appendChild(node);
    if (state.pinned) followNewest();
    el.thread.hidden = false;
    el.foot.hidden = false;
    el.card.dataset.thread = 'open';
    layout();
    return node;
  }

  // A question whose turn failed stays in the thread as it was asked: tinted when no reply came, and
  // followed by its error line (after the part of a reply that was kept, if any).
  function markFailed(userMsg, message, replyMsg) {
    if (!replyMsg) userMsg.node.classList.add('failed');
    const line = addError(message, userMsg.content);
    (replyMsg || userMsg).node.after(line);
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
            // Always the provider chosen: there is no fallback (ADR 0014). The reply's chip names it
            // and its model, as a reopened conversation will.
            setProvider(reply, { id: m.id, name: m.name, model: m.model });
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
          case 'error': {
            // A reply that was cut short keeps what it wrote. A question the server took is marked
            // failed, as a reopened conversation will show it; one it never took moves to "Not sent".
            const partial = reply.content.trim() !== '';
            if (!partial) removeMessage(reply);
            if (accepted && userMsg) markFailed(userMsg, m.message, partial ? reply : null);
            else addError(m.message);
            checkStatus();
            finish();
            break;
          }
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

  async function send() {
    const text = el.input.value.trim();
    if (!text || state.streaming) return;
    el.input.value = '';
    autosize();
    await askText(text);
  }

  // Asks `text` in the open conversation: the question, then its reply as it streams.
  async function askText(text) {
    hideNotice();
    state.pinned = true;
    const userMsg = addMessage('user', text);
    const reply = addMessage('assistant', '', { streaming: true });
    await streamInto(reply, (channel) => invoke('ask', { conversationId: state.convId, text, model: state.model, onEvent: channel }), userMsg);
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
      if (state.pinned) followNewest();
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

      const copy = document.createElement('button');
      copy.type = 'button';
      copy.className = 'copy-conv';
      copy.title = 'Copy conversation';
      copy.setAttribute('aria-label', 'Copy conversation: ' + c.title);

      row.append(open, copy, del);
      el.histList.appendChild(row);
    }
  }

  // A whole conversation as Markdown: its title, then each message in order with who wrote it and
  // when; a reply also names its model and provider, and a failed question says why.
  function conversationMarkdown(title, rows) {
    const parts = ['# ' + title];
    for (const r of rows) {
      let head = (r.role === 'user' ? '**You**' : '**Assistant**') + ' · ' + formatTime(r.created_at);
      if (r.provider) head += ' · ' + r.provider.model + ' via ' + r.provider.name;
      parts.push(head, r.content);
      if (r.failed) parts.push('> Failed: ' + r.failed.message);
    }
    return parts.join('\n\n') + '\n';
  }

  async function copyConversation(id, button) {
    let rows;
    try {
      rows = await invoke('load_conversation', { id });
    } catch (e) {
      showNotice(String(e));
      return;
    }
    const c = state.conversations.find((x) => String(x.id) === id);
    await copyText(conversationMarkdown(c ? c.title : 'Conversation', rows), button);
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
    // The conversation remembers the model its latest message asked for (if it is still approved).
    const chat = state.conversations.find((x) => String(x.id) === String(id));
    state.model = chat && state.models.some((m) => m.id === chat.model) ? chat.model : null;
    syncModelPicker();
    const msgs = rows.map((r) => addMessage(r.role, r.content, { createdAt: r.created_at, provider: r.provider }));
    rows.forEach((r, i) => {
      if (r.role !== 'user' || !r.failed) return;
      const next = rows[i + 1] && rows[i + 1].role === 'assistant' ? msgs[i + 1] : null;
      markFailed(msgs[i], r.failed.message, next);
    });
    state.pinned = true;
    followNewest();
    renderHistory();
    el.input.focus();
    syncUnsent();
    syncPending();
  }

  function newChat() {
    if (state.streaming) return;
    state.convId = null;
    state.model = null;
    syncModelPicker();
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
      if (closeLinkConfirm() || closePairing() || closeShare()) return;
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
    state.pinned = state.newestFirst ? t.scrollTop < 40 : t.scrollHeight - t.scrollTop - t.clientHeight < 40;
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
      askToOpen(link.getAttribute('href'));
    }
  });

  el.linkOpen.addEventListener('click', openPendingLink);
  el.linkCancel.addEventListener('click', closeLinkConfirm);

  el.histList.addEventListener('click', (e) => {
    const t = e.target;
    if (!(t instanceof Element)) return;
    const row = t.closest('.conv');
    if (!row) return;
    const id = row.dataset.id;
    if (t.closest('.del')) onDelete(id);
    else if (t.closest('.copy-conv')) copyConversation(id, t.closest('.copy-conv'));
    else if (t.closest('.open')) openConversation(id);
  });

  el.histToggle.addEventListener('click', () => toggleHistory());
  el.newChat.addEventListener('click', newChat);
  el.stop.addEventListener('click', cancel);
  el.noticeClose.addEventListener('click', hideNotice);
  el.pair.addEventListener('submit', submitPairing);
  el.pairCancel.addEventListener('click', closePairing);
  el.share.addEventListener('submit', submitShare);
  el.shareCancel.addEventListener('click', closeShare);
  el.shareCopy.addEventListener('click', () => copyText(el.shareKey.value, el.shareCopy));
  el.noticeConfig.addEventListener('click', () => invoke('open_config').catch(() => {}));

  new ResizeObserver(layout).observe(el.card);

  // ---------------------------------------------------------------- boot

  // The model picker (ADR 0014): shown when there is a choice, with the model this chat uses selected. A chat that
  // picked none uses the one chosen for new chats, which the server remembers for all of the person's apps.
  async function refreshModels() {
    let picker;
    try { picker = await invoke('list_models'); } catch (_) { return; }
    state.models = picker.models || [];
    state.activeModel = picker.active || null;
    if (state.model && !state.models.some((m) => m.id === state.model)) state.model = null;
    el.modelPicker.textContent = '';
    for (const m of state.models) {
      const o = document.createElement('option');
      o.value = m.id;
      o.textContent = m.label;
      el.modelPicker.appendChild(o);
    }
    syncModelPicker();
  }

  function syncModelPicker() {
    const several = state.models.length > 1;
    el.modelPicker.hidden = !several;
    el.modelLabel.hidden = several;
    if (several) el.modelPicker.value = state.model || state.activeModel || state.models[0].id;
    syncChrome();
  }

  el.modelPicker.addEventListener('change', async () => {
    state.model = el.modelPicker.value;
    // Also what new chats use from now on, on every app of this person.
    try { await invoke('set_active_model', { id: state.model }); state.activeModel = state.model; } catch (e) { showNotice(String(e)); }
  });

  // ------------------------------------------------- privacy notice, AI warning

  // What the server tells this app to say about the service (ADR 0012). Consent is assumed when the administrator makes a user, so
  // nothing is sent back: the app only remembers the version of the notice it last showed, here, and shows it again when the
  // version differs and right after pairing. Everything goes in as text.
  const NOTICE_KEY = 'askbar.privacyNoticeVersion';
  function shownVersion() {
    try { return window.localStorage.getItem(NOTICE_KEY); } catch (_) { return null; }
  }
  function rememberVersion(v) {
    try { window.localStorage.setItem(NOTICE_KEY, v); } catch (_) { /* shown again next time */ }
  }
  function forgetVersion() {
    try { window.localStorage.removeItem(NOTICE_KEY); } catch (_) { /* nothing to forget */ }
  }

  function applyPolicy(policy) {
    if (!policy) return; // the server was not asked, or does not send one: keep what is shown
    const w = policy.ai_warning;
    const warn = !!(w && w.enabled && w.text);
    el.aiWarning.textContent = warn ? w.text : '';
    el.aiWarning.hidden = !warn;
    const n = policy.notice;
    const due = !!(n && n.enabled && n.text && n.version && n.version !== shownVersion());
    if (due) {
      el.privacyText.textContent = n.text;
      el.privacyDays.textContent = 'Audit records are kept for ' + n.retention_days + (n.retention_days === 1 ? ' day.' : ' days.');
      state.noticeVersion = n.version;
    }
    el.privacy.hidden = !due;
    layout();
  }

  el.privacyOk.addEventListener('click', () => {
    if (state.noticeVersion) rememberVersion(state.noticeVersion);
    el.privacy.hidden = true;
    layout();
    el.input.focus();
  });

  async function refreshBootstrap() {
    const b = await invoke('bootstrap');
    state.ui = b.ui;
    applyUi(b.ui);
    applyPolicy(b.policy);
    state.provider = b.provider;
    el.modelLabel.textContent = b.model ? b.model + ' via ' + b.provider : '';
    if (b.notice) showNotice(b.notice);
    await refreshModels();
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
  listen('askbar:pair-another', () => openShare());

  (async () => {
    try { await refreshBootstrap(); } catch (e) { showNotice(String(e)); }
    checkStatus();
    await refreshHistory();
    syncChrome();
    syncUnsent();
    el.input.focus();
  })();
})();
