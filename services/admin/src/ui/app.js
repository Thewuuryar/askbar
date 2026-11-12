// Askbar admin. Plain JavaScript, no build step. Everything user-supplied goes in via textContent.
(() => {
  'use strict';

  const $ = (id) => document.getElementById(id);
  const show = (node, visible) => { node.hidden = !visible; };
  // Each page holds only its own section (plan step 1.16), so wiring for another page's elements is skipped.
  const on = (id, event, fn) => { const el = $(id); if (el) el.addEventListener(event, fn); };

  async function api(method, path, body) {
    const res = await fetch(path, {
      method,
      credentials: 'same-origin',
      headers: { 'X-Askbar-Admin': '1', ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}) },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    if (res.status === 401 && !(method === 'POST' && path === '/api/admin/v1/session')) { showLogin(); throw new Error('Signed out'); }
    let data = null;
    const text = await res.text();
    if (text) { try { data = JSON.parse(text); } catch (_) { data = null; } }
    if (res.status === 502 && typeof show === 'function' && document.getElementById('offline')) show($('offline'), true);
    if (!res.ok) {
      // A problem+json body: `detail` is the sentence for a person, `error` the stable code.
      const err = new Error((data && (data.detail || data.error)) || ('Request failed (' + res.status + ')'));
      err.status = res.status;
      err.code = (data && data.error) || null;
      throw err;
    }
    return data;
  }

  function flash(text) {
    const f = $('flash');
    f.textContent = text;
    show(f, !!text);
  }

  // ------------------------------------------------------------- pages

  // One admin page per URL (plan step 1.16): the nav is plain links, and this script runs only the
  // current page's loader. A page with no entry here needs none; audit.js registers the Audit page's.
  const page = document.body.dataset.page;
  const tabs = [
    { id: 'providers', load: loadProviders },
    { id: 'services', load: loadServices },
    { id: 'clients', load: loadClients },
    { id: 'settings', load: loadSettings },
    { id: 'network', load: loadNetwork },
    { id: 'security', load: loadKeys },
  ];
  let appShown = false;
  function registerTab(tab) {
    tabs.push(tab);
    if (appShown && tab.id === page) loadPage();
  }
  window.askbarAdmin = { api, flash, registerTab, show, $ };

  function loadPage() {
    flash('');
    const t = tabs.find((x) => x.id === page);
    if (t && t.load) t.load().catch((e) => flash(e.message));
  }

  // ------------------------------------------------------------ login

  function hideAll() {
    for (const id of ['app', 'login', 'setup']) show($(id), false);
  }

  // First run (ADR 0007): signed in with the published password, change it before anything else.
  function showPasswordChange() {
    hideAll();
    show($('setup'), true);
    show($('setup-audit-form'), false);
    show($('setup-audit-done'), false);
    show($('setup-password-form'), true);
    $('setup-current').focus();
  }

  async function route() {
    let signedIn = true;
    try { await api('GET', '/api/admin/v1/session'); } catch (_) { signedIn = false; }
    if (!signedIn) return showLogin();
    let status;
    try { status = await api('GET', '/api/admin/v1/setup'); } catch (_) { status = {}; }
    if (status.password_change_required) return showPasswordChange();
    // The audit passphrase is part of setup: without a key the server records nothing it may keep,
    // and refuses to start replies. An install from before audit keys is asked on its next visit.
    if (status.audit_key_set === false) return showAuditSetup();
    showApp();
  }

  function showAuditSetup() {
    hideAll();
    show($('setup'), true);
    show($('setup-password-form'), false);
    show($('setup-audit-done'), false);
    show($('setup-audit-form'), true);
    $('setup-audit').focus();
  }

  on('setup-audit-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('setup-audit-error');
    show(err, false);
    const fail = (m) => { err.textContent = m; show(err, true); };
    const pass = $('setup-audit').value;
    if (pass !== $('setup-audit2').value) return fail('The two passphrases do not match.');
    if ([...pass].length < 20) return fail('The passphrase must be at least 20 characters.');
    if (!$('setup-audit-stored').checked) return fail('Confirm that you stored the passphrase: it cannot be recovered.');
    try {
      const r = await api('PUT', '/api/admin/v1/setup/audit-key', { passphrase: pass, confirm: $('setup-audit2').value });
      $('setup-audit').value = '';
      $('setup-audit2').value = '';
      $('setup-audit-stored').checked = false;
      $('setup-audit-keyid').textContent = r.key_id;
      show($('setup-audit-form'), false);
      show($('setup-audit-done'), true);
    } catch (ex) {
      fail(ex.message);
    }
  });
  on('setup-audit-continue', 'click', () => route());

  on('setup-password-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('setup-password-error');
    show(err, false);
    if ($('setup-password').value !== $('setup-password2').value) {
      err.textContent = 'The two passwords do not match.';
      show(err, true);
      return;
    }
    try {
      await api('PUT', '/api/admin/v1/password', { current: $('setup-current').value, new: $('setup-password').value });
      for (const id of ['setup-current', 'setup-password', 'setup-password2']) $(id).value = '';
      route();
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  function showLogin() {
    hideAll();
    show($('login'), true);
    $('password').value = '';
    $('password').focus();
  }

  // Sealed content (ADR 0011): a banner with the time left and a Seal button while it is open, and the alerts of the last
  // day (an unseal, a change of the timeouts). Everything is set as text.
  let sealedLeft = 0;
  function minutes(secs) { return Math.max(1, Math.ceil(secs / 60)); }
  function showSealed(s) {
    const open = !!s.open;
    sealedLeft = open ? Math.min(s.idle_left_secs || 0, s.total_left_secs || 0) : 0;
    $('unsealed-text').textContent = open ? 'Sealed content is open for about ' + minutes(sealedLeft) + ' more minute' + (minutes(sealedLeft) === 1 ? '' : 's') + ' (less if it is not used).' : '';
    show($('unsealed-banner'), open);
    const box = $('sealed-alerts');
    box.textContent = '';
    for (const a of s.alerts || []) {
      const p = document.createElement('p');
      p.className = 'flash';
      p.dataset.kind = a.kind;
      p.textContent = new Date(a.at * 1000).toLocaleString() + ' · ' + a.text;
      box.appendChild(p);
    }
    if (window.askbarAudit && window.askbarAudit.sessionFromStatus) window.askbarAudit.sessionFromStatus(s);
  }
  $('unsealed-seal').addEventListener('click', async () => {
    try { await api('DELETE', '/api/admin/v1/audit/session'); } catch (_) { /* signed out */ }
    checkServer();
  });

  // Alert delivery (ADR 0055): a banner when no channel is set up, when delivery has not worked for half an hour, or when the
  // alerter does not answer. The sentences are the admin's own, set as text.
  const ALERTS_BANNER = {
    not_set_up: 'Alerts are not set up, so nothing will tell you when something needs attention. Set up ntfy under Settings, then send a test alert.',
    failing: 'Alert delivery has not worked for half an hour. Check the ntfy settings under Settings and send a test alert.',
    unreachable: 'The alerter does not answer, so no alert is delivered. Look at it on the Services page.',
  };
  function showAlertsBanner(state) {
    const text = ALERTS_BANNER[state] || '';
    $('alerts-banner').textContent = text;
    show($('alerts-banner'), !!text);
  }

  // The "server offline" banner: ask the admin how its link to the server is, now and then.
  let statusTimer = null;
  async function checkServer() {
    try {
      const st = await api('GET', '/api/admin/v1/status');
      // A refused token is the more useful message: the server is there, but it does not accept this admin.
      show($('rejected'), !!st.token_rejected);
      show($('offline'), !st.server_online && !st.token_rejected);
      // The audit chain no longer matches what this admin stored (ADR 0010 rule 7). The sentence is the admin's, text only.
      const chain = st.audit_chain || {};
      const bad = ['rollback', 'rewritten', 'edited'].includes(chain.state);
      $('chain-alert').textContent = bad ? chain.detail : '';
      show($('chain-alert'), bad);
      showAlertsBanner(st.alerts);
      showSealed(st.sealed_content || {});
    } catch (_) { /* signed out, or the admin itself is away */ }
  }

  function showApp() {
    hideAll();
    show($('app'), true);
    appShown = true;
    loadPage();
    checkServer();
    if (!statusTimer) statusTimer = setInterval(() => { if (!$('app').hidden) checkServer(); }, 5000);
  }

  on('login-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('login-error');
    show(err, false);
    try {
      await api('POST', '/api/admin/v1/session', { username: $('username').value, password: $('password').value });
      route();
    } catch (ex) {
      err.textContent = ex.status === 429 ? 'Too many attempts. Try again later.' : 'Wrong username or password.';
      show(err, true);
    }
  });

  on('logout', 'click', async () => {
    try { await api('DELETE', '/api/admin/v1/session'); } catch (_) { /* already signed out */ }
    route();
  });

  // -------------------------------------------------------- services

  const SERVICE_INFO = {
    server: { title: 'Server', what: 'the Askbar server (chats, history, provider calls)', warn: 'Replies being written end as interrupted, and apps say "server offline" until it is back (under a minute).' },
    alerter: { title: 'Alerter', what: 'the alerter (delivers alerts)', warn: 'Alerts wait or are lost for a few seconds, and the admin gives it its delivery settings again.' },
    'model-gateway': { title: 'Model gateway', what: 'the model gateway (provider calls and provider keys)', warn: 'Its provider keys leave its memory, and the admin gives them back within seconds.' },
    admin: { title: 'Admin', what: 'the admin (this site)', warn: 'This page stops answering for a few seconds, then reloads, and you sign in again.' },
    caddy: { title: 'Caddy (HTTPS front end)', what: 'the HTTPS front end', warn: 'Its configuration is pushed again; connections may drop for a moment.' },
    cpa: { title: 'CLIProxyAPI', what: 'CLIProxyAPI', warn: 'Providers that use your Claude or ChatGPT login fail for a few seconds while it starts again.' },
  };
  let servicesBusy = false;

  function serviceCard(svc) {
    const info = SERVICE_INFO[svc.name] || { title: svc.name, what: svc.name, warn: '' };
    const card = document.createElement('div');
    card.className = 'card';
    card.dataset.name = svc.name;
    const head = document.createElement('div');
    head.className = 'row';
    const title = document.createElement('strong');
    title.textContent = info.title;
    const state = document.createElement('span');
    state.className = 'svc-state';
    state.textContent = svc.state + (svc.health ? ' · ' + svc.health : '');
    head.append(title, state);
    const meta = document.createElement('div');
    meta.className = 'hint';
    meta.textContent = svc.status || '';
    const buttons = document.createElement('div');
    buttons.className = 'row';
    for (const action of ['restart']) {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'secondary';
      b.dataset.action = action;
      b.textContent = action[0].toUpperCase() + action.slice(1);
      b.disabled = servicesBusy;
      b.addEventListener('click', () => serviceAction(svc.name, action, info));
      buttons.appendChild(b);
    }
    const out = document.createElement('span');
    out.className = 'svc-result hint';
    buttons.appendChild(out);
    card.append(head, meta, buttons);
    return card;
  }

  async function serviceAction(name, action, info) {
    const sentence = 'Restart ' + info.what + '? ' + info.warn;
    if (!confirm(sentence.trim())) return;
    servicesBusy = true;
    const card = document.querySelector('#services .card[data-name="' + name + '"]');
    for (const b of document.querySelectorAll('#services button')) b.disabled = true;
    const out = card && card.querySelector('.svc-result');
    if (out) out.textContent = 'Working...';
    try {
      await api('POST', '/api/admin/v1/services/' + encodeURIComponent(name) + '/' + action);
      if (name === 'admin') {
        // It exits after answering; wait until it answers again, then start the page afresh.
        if (out) out.textContent = 'Restarting...';
        for (let i = 0; i < 60; i++) {
          await new Promise((r) => setTimeout(r, 1000));
          try { const r = await fetch('/api/admin/v1/health'); if (r.ok && i > 1) { location.reload(); return; } } catch (_) { /* still down */ }
        }
        flash('The admin has not come back within a minute. Check it from the host.');
      }
    } catch (ex) {
      flash(ex.message);
    }
    servicesBusy = false;
    loadServices().catch((e) => flash(e.message));
  }

  async function loadServices() {
    const box = $('services');
    let data;
    try { data = await api('GET', '/api/admin/v1/services'); }
    catch (ex) {
      box.textContent = '';
      const p = document.createElement('p');
      p.className = 'error';
      p.textContent = ex.message;
      box.appendChild(p);
      return;
    }
    box.textContent = '';
    for (const svc of data.services) box.appendChild(serviceCard(svc));
    loadCpa();
    const rec = $('services-recent');
    rec.textContent = data.recent.length ? '' : 'None yet.';
    for (const r of data.recent) {
      const line = document.createElement('div');
      line.textContent = new Date(r.at * 1000).toLocaleString() + ' · ' + r.action + ' ' + r.service + (r.ok ? '' : ' · failed');
      rec.appendChild(line);
    }
  }
  // ---- CLIProxyAPI accounts: connect Claude or ChatGPT without a terminal.
  let login = null; // { state } while a login is open

  function cpaMessage(text) { $('cpa-message').textContent = text || ''; }

  function closeLogin() {
    login = null;
    $('cpa-redirect').value = '';
    show($('cpa-login'), false);
  }

  async function loadCpa() {
    const panel = $('cpa-panel');
    let c;
    try { c = await api('GET', '/api/admin/v1/cpa'); } catch (_) { show(panel, false); return; }
    show(panel, c.configured);
    if (!c.configured) return;
    const box = $('cpa-accounts');
    box.textContent = '';
    if (!c.reachable) cpaMessage('CLIProxyAPI is not answering. Start it with the card above.');
    for (const a of c.accounts) {
      const row = document.createElement('div');
      row.className = 'row';
      const label = document.createElement('span');
      label.textContent = (a.provider === 'anthropic' ? 'Claude' : 'ChatGPT') + ' · ' + a.label;
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'secondary';
      b.dataset.action = 'disconnect';
      b.textContent = 'Disconnect';
      b.addEventListener('click', async () => {
        if (!confirm('Disconnect ' + a.label + '? Replies through the CPA provider stop working until you connect again.')) return;
        try { await api('DELETE', '/api/admin/v1/cpa/accounts/' + encodeURIComponent(a.name)); } catch (ex) { flash(ex.message); }
        loadCpa();
      });
      row.append(label, b);
      box.appendChild(row);
    }
    if (!c.accounts.length && c.reachable) { cpaMessage('No account connected yet.'); }
  }

  async function connect(provider) {
    cpaMessage('');
    try {
      const r = await api('POST', '/api/admin/v1/cpa/logins', { provider });
      login = { state: r.state };
      const a = $('cpa-login-link');
      a.textContent = r.url;
      a.href = r.url;
      show($('cpa-login'), true);
      $('cpa-redirect').focus();
    } catch (ex) { cpaMessage(ex.message); }
  }
  on('cpa-connect-anthropic', 'click', () => connect('anthropic'));
  on('cpa-connect-openai', 'click', () => connect('openai'));
  on('cpa-cancel', 'click', closeLogin);

  on('cpa-finish', 'click', async () => {
    if (!login) return;
    const state = login.state;
    cpaMessage('Sending...');
    try {
      await api('POST', '/api/admin/v1/cpa/logins/' + encodeURIComponent(state) + '/callback', { redirect_url: $('cpa-redirect').value.trim() });
    } catch (ex) { cpaMessage(ex.message); return; }
    closeLogin();
    cpaMessage('Waiting for CLIProxyAPI to confirm...');
    for (let i = 0; i < 30; i++) {
      let r;
      try { r = await api('GET', '/api/admin/v1/cpa/logins/' + encodeURIComponent(state)); } catch (ex) { cpaMessage(ex.message); return; }
      if (r.status === 'ok') { cpaMessage('Connected.' + (r.provider_id ? ' Pick the model on the CPA provider (Providers tab).' : '')); loadCpa(); return; }
      if (r.status === 'error') { cpaMessage('The login failed' + (r.message ? ': ' + r.message : '.')); return; }
      await new Promise((res) => setTimeout(res, 2000));
    }
    cpaMessage('CLIProxyAPI has not confirmed yet. Check the accounts list in a moment.');
  });

  // Keep the state fresh while the tab is on screen.
  setInterval(() => {
    const sec = document.querySelector('section[data-tab="services"]');
    if (sec && !sec.hidden && !servicesBusy && !document.hidden) loadServices().catch(() => {});
  }, 5000);

  // -------------------------------------------------------- providers

  let editing = null; // provider id, or null when adding

  // The details of a provider test, folded away until asked for: the request's method and path, the status, the error
  // type and the timings. The server never sends a key, a reply or a provider's own words, and this shows only text.
  function testDetails(d) {
    const box = document.createElement('details');
    box.className = 'test-details';
    const sum = document.createElement('summary');
    sum.textContent = 'Details';
    const dl = document.createElement('dl');
    const row = (k, v) => {
      if (v === null || v === undefined) return;
      const dt = document.createElement('dt');
      dt.textContent = k;
      const dd = document.createElement('dd');
      dd.textContent = String(v);
      dl.append(dt, dd);
    };
    row('Request', d.method + ' ' + d.path);
    row('Status', d.status);
    row('Error type', d.error_type);
    row('Time to first text', d.ttft_ms === null || d.ttft_ms === undefined ? null : d.ttft_ms + ' ms');
    row('Total time', d.total_ms + ' ms');
    box.append(sum, dl);
    return box;
  }

  function providerCard(p) {
    const card = document.createElement('div');
    card.className = 'card';
    card.dataset.name = p.name;

    const head = document.createElement('div');
    head.className = 'row';
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = p.name;
    const badge = document.createElement('span');
    badge.className = 'badge';
    badge.textContent = p.type;
    const key = document.createElement('span');
    key.className = 'badge';
    key.textContent = p.key_missing ? 'key missing on the server, enter it again' : p.has_key ? 'key set' : 'no key';
    if (p.key_missing) key.classList.add('error');
    const sp = document.createElement('span');
    sp.className = 'spacer';
    head.append(name, badge, key, sp);

    const test = button('Test', 'secondary', async () => {
      result.className = 'test-result';
      result.textContent = 'Testing…';
      test.disabled = true;
      try {
        const r = await api('POST', '/api/admin/v1/providers/' + encodeURIComponent(p.id) + '/test');
        result.className = 'test-result ' + (r.ok ? 'ok' : 'error');
        result.textContent = '';
        const line = document.createElement('span');
        line.className = 'test-line';
        line.textContent = (r.ok ? 'Success: ' : 'Failed: ') + r.message;
        result.appendChild(line);
        if (r.details) result.appendChild(testDetails(r.details));
      } catch (ex) {
        result.className = 'test-result error';
        result.textContent = 'Failed: ' + ex.message;
      }
      test.disabled = false;
    });
    test.dataset.action = 'test';
    const edit = button('Edit', 'secondary', () => openForm(p));
    edit.dataset.action = 'edit';
    const del = button('Delete', 'danger', async () => {
      if (!confirm('Delete provider "' + p.name + '"?')) return;
      try { await api('DELETE', '/api/admin/v1/providers/' + encodeURIComponent(p.id)); loadProviders(); } catch (ex) { flash(ex.message); }
    });
    del.dataset.action = 'delete';
    if (p.managed) {
      const m = document.createElement('span');
      m.className = 'badge';
      m.textContent = 'managed by the admin';
      head.insertBefore(m, sp);
      head.append(test, edit);
    } else {
      head.append(test, edit, del);
    }

    const meta = document.createElement('div');
    meta.className = 'meta';
    const prices = [['in', p.price_in], ['out', p.price_out], ['cached in', p.price_cached_in]].filter(([, v]) => v != null).map(([k, v]) => k + ' ' + v);
    meta.textContent = p.model + (p.base_url ? ' · ' + p.base_url : '') +
      ' · ' + (p.billing === 'subscription' ? 'subscription' : 'metered') +
      (prices.length ? ' · per million: ' + prices.join(', ') : '');
    const result = document.createElement('div');
    result.className = 'test-result';
    card.append(head, meta, result);
    return card;
  }

  function button(label, cls, onClick) {
    const b = document.createElement('button');
    b.type = 'button';
    if (cls) b.className = cls;
    b.textContent = label;
    b.addEventListener('click', onClick);
    return b;
  }

  async function loadProviders() {
    const list = await api('GET', '/api/admin/v1/providers');
    const box = $('providers');
    box.textContent = '';
    if (!list.length) {
      const p = document.createElement('p');
      p.className = 'hint';
      p.textContent = 'No providers yet. Add one so clients have something to talk to.';
      box.appendChild(p);
    }
    for (const p of list) box.appendChild(providerCard(p));
    providersById = new Map(list.map((p) => [p.id, p]));
    await loadCatalog();
  }

  // ---- approved models (ADR 0014): the models people may use, and the default. A draft is edited here and saved whole.
  let providersById = new Map();
  let catalogDraft = { models: [], default: null };
  let catalogOffered = null; // { providerId, models: [name] } after "Load models"

  const modelId = (m) => m.provider_id + '/' + m.model;
  const providerName = (id) => (providersById.get(id) ? providersById.get(id).name : 'a provider that is gone');

  async function loadCatalog() {
    if (!$('catalog-section')) return;
    catalogDraft = await api('GET', '/api/admin/v1/catalog');
    catalogOffered = null;
    const sel = $('catalog-provider');
    const keep = sel.value;
    sel.textContent = '';
    for (const p of providersById.values()) {
      const o = document.createElement('option');
      o.value = p.id;
      o.textContent = p.name;
      sel.appendChild(o);
    }
    if (keep && providersById.has(keep)) sel.value = keep;
    show($('catalog-error'), false);
    $('catalog-status').textContent = '';
    renderCatalog();
  }

  function setApproved(m, approved) {
    const id = modelId(m);
    const has = catalogDraft.models.some((x) => modelId(x) === id);
    if (approved && !has) catalogDraft.models.push({ provider_id: m.provider_id, model: m.model });
    if (!approved && has) catalogDraft.models = catalogDraft.models.filter((x) => modelId(x) !== id);
    // One default whenever anything is approved: the first one if there was none (or it was just removed).
    if (!catalogDraft.models.length) catalogDraft.default = null;
    else if (!catalogDraft.models.some((x) => modelId(x) === catalogDraft.default)) catalogDraft.default = modelId(catalogDraft.models[0]);
    renderCatalog();
  }

  function renderCatalog() {
    const box = $('catalog-approved');
    box.textContent = '';
    if (!catalogDraft.models.length) {
      const p = document.createElement('p');
      p.className = 'hint';
      p.textContent = 'No model is approved yet, so nothing answers. Load a provider\'s models below and tick the ones to approve.';
      box.appendChild(p);
    }
    for (const m of catalogDraft.models) {
      const row = document.createElement('div');
      row.className = 'row';
      row.dataset.model = modelId(m);
      const label = document.createElement('label');
      const radio = document.createElement('input');
      radio.type = 'radio';
      radio.name = 'catalog-default';
      radio.checked = catalogDraft.default === modelId(m);
      radio.addEventListener('change', () => { catalogDraft.default = modelId(m); renderCatalog(); });
      const text = document.createElement('span');
      text.textContent = ' ' + providerName(m.provider_id) + ' / ' + m.model + (radio.checked ? ' (default)' : '');
      label.append(radio, text);
      const remove = document.createElement('button');
      remove.type = 'button';
      remove.className = 'secondary';
      remove.textContent = 'Remove';
      remove.addEventListener('click', () => setApproved(m, false));
      row.append(label, remove);
      box.appendChild(row);
    }
    const offered = $('catalog-offered');
    offered.textContent = '';
    if (catalogOffered) {
      for (const name of catalogOffered.models) {
        const m = { provider_id: catalogOffered.providerId, model: name };
        const label = document.createElement('label');
        label.className = 'row';
        const box1 = document.createElement('input');
        box1.type = 'checkbox';
        box1.checked = catalogDraft.models.some((x) => modelId(x) === modelId(m));
        box1.addEventListener('change', () => setApproved(m, box1.checked));
        const text = document.createElement('span');
        text.textContent = ' ' + name;
        label.append(box1, text);
        offered.appendChild(label);
      }
    }
  }

  on('catalog-load', 'click', async () => {
    const p = providersById.get($('catalog-provider').value);
    const status = $('catalog-status');
    if (!p) { status.textContent = 'Add a provider first.'; return; }
    status.textContent = 'Loading models...';
    let res;
    try { res = await api('POST', '/api/admin/v1/providers/models', { type: p.type, base_url: p.base_url, provider_id: p.id }); }
    catch (ex) { res = { ok: false, message: ex.message }; }
    if (res.ok && res.models.length) {
      catalogOffered = { providerId: p.id, models: res.models.map((m) => m.id) };
      status.textContent = res.models.length + ' models offered by ' + p.name + '.';
    } else {
      catalogOffered = null;
      status.textContent = res.message || 'The provider listed no models.';
    }
    renderCatalog();
  });

  on('catalog-save', 'click', async () => {
    const err = $('catalog-error');
    show(err, false);
    try {
      await api('PUT', '/api/admin/v1/catalog', catalogDraft);
      flash('');
      await loadCatalog();
      $('catalog-status').textContent = 'Saved.';
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  on('catalog-discard', 'click', () => { loadCatalog(); });

  // ---- model list: once the provider can be reached, the model is chosen from what it offers.
  const MANUAL = '__manual__';
  let editingHasKey = false;
  let modelSeq = 0;

  function canListModels() {
    if ($('p-key').value.trim() !== '' || (editing && editingHasKey)) return true;
    return $('p-type').value === 'openai' && $('p-base-url').value.trim() !== '';
  }

  function showModelInput() {
    show($('p-model-list'), false);
    show($('p-model'), true);
  }

  function fillModelList(models) {
    const sel = $('p-model-list');
    const current = $('p-model').value.trim();
    sel.textContent = '';
    const add = (value, text) => { const o = document.createElement('option'); o.value = value; o.textContent = text; sel.appendChild(o); };
    if (current && !models.some((m) => m.id === current)) add(current, current + ' (not in the provider\'s list)');
    for (const m of models) add(m.id, m.label ? m.label + ' (' + m.id + ')' : m.id);
    add(MANUAL, 'Enter a name myself...');
    sel.value = current || models[0].id;
    $('p-model').value = sel.value;
    show($('p-model'), false);
    show(sel, true);
  }

  async function loadModels() {
    const status = $('p-model-status');
    if (!canListModels()) {
      showModelInput();
      status.textContent = 'Enter the address and key, then load the provider\'s models.';
      return;
    }
    const seq = ++modelSeq;
    status.textContent = 'Loading models...';
    const body = { type: $('p-type').value, base_url: $('p-base-url').value.trim() };
    if ($('p-key').value.trim() !== '') body.api_key = $('p-key').value.trim();
    if (editing) body.provider_id = editing;
    let res;
    try { res = await api('POST', '/api/admin/v1/providers/models', body); }
    catch (ex) { res = { ok: false, message: ex.message }; }
    if (seq !== modelSeq) return; // a newer request replaced this one
    if (res.ok && res.models.length) {
      fillModelList(res.models);
      status.textContent = res.models.length + ' models available.';
    } else {
      showModelInput();
      status.textContent = (res.message || 'No models listed.') + ' Type the model name instead.';
    }
  }

  on('p-model-list', 'change', () => {
    if ($('p-model-list').value === MANUAL) { showModelInput(); $('p-model').focus(); return; }
    $('p-model').value = $('p-model-list').value;
  });
  on('p-model-refresh', 'click', loadModels);
  for (const id of ['p-type', 'p-base-url', 'p-key']) on(id, 'change', loadModels);

  function openForm(p, preset) {
    editing = p ? p.id : null;
    editingHasKey = !!(p && p.has_key);
    const v = p || preset || { name: '', type: 'openai', base_url: '', model: '' };
    $('provider-form-title').textContent = p ? 'Edit ' + p.name : 'Add provider';
    $('p-name').value = v.name || '';
    $('p-type').value = v.type || 'openai';
    $('p-base-url').value = v.base_url || '';
    $('p-model').value = v.model || '';
    // A managed provider (CLIProxyAPI) keeps its name, type, address and key.
    for (const id of ['p-name', 'p-type', 'p-base-url', 'p-key']) $(id).disabled = !!(p && p.managed);
    $('p-key').value = ''; // never prefilled
    $('p-key').placeholder = p && p.has_key ? 'unchanged (write-only)' : 'write-only';
    $('p-max-tokens').value = v.max_tokens || '';
    $('p-temperature').value = v.temperature == null ? '' : v.temperature;
    $('p-billing').value = v.billing || 'metered';
    $('p-price-in').value = v.price_in == null ? '' : v.price_in;
    $('p-price-out').value = v.price_out == null ? '' : v.price_out;
    $('p-price-cached').value = v.price_cached_in == null ? '' : v.price_cached_in;
    show($('provider-error'), false);
    $('p-hint').textContent = preset && preset.hint ? preset.hint : '';
    show($('p-hint'), !!(preset && preset.hint));
    show($('provider-form'), true);
    modelSeq++;
    showModelInput();
    $('p-model-status').textContent = '';
    if (p) loadModels();
    $('p-name').focus();
  }

  function closeForm() {
    editing = null;
    modelSeq++;
    $('p-key').value = '';
    show($('provider-form'), false);
  }

  on('add-provider', 'click', () => openForm(null));
  on('provider-cancel', 'click', closeForm);

  on('provider-form', 'submit', async (e) => {
    e.preventDefault();
    const num = (id, parse) => ($(id).value.trim() === '' ? null : parse($(id).value));
    const body = {
      name: $('p-name').value.trim(),
      type: $('p-type').value,
      base_url: $('p-base-url').value.trim(),
      model: $('p-model').value.trim(),
      api_key: $('p-key').value,
      max_tokens: num('p-max-tokens', (x) => parseInt(x, 10)),
      temperature: num('p-temperature', parseFloat),
      billing: $('p-billing').value,
      price_in: num('p-price-in', parseFloat),
      price_out: num('p-price-out', parseFloat),
      price_cached_in: num('p-price-cached', parseFloat),
    };
    try {
      if (editing) await api('PUT', '/api/admin/v1/providers/' + encodeURIComponent(editing), body);
      else await api('POST', '/api/admin/v1/providers', body);
      closeForm();
      loadProviders();
    } catch (ex) {
      $('provider-error').textContent = ex.message;
      show($('provider-error'), true);
    }
  });

  // --------------------------------------------------------- security (admin password)

  on('password-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('pw-error');
    show(err, false);
    if ($('pw-new').value !== $('pw-new2').value) {
      err.textContent = 'The two new passwords do not match.';
      show(err, true);
      return;
    }
    try {
      await api('PUT', '/api/admin/v1/password', { current: $('pw-current').value, new: $('pw-new').value });
      for (const id of ['pw-current', 'pw-new', 'pw-new2']) $(id).value = '';
      flash('Password changed. Other browsers have been signed out.');
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  // ------------------------------------------------------ admin API key

  let keyId = null;
  let revokeArmed = null;

  function ago(t) {
    if (!t) return 'never used';
    return 'last used ' + new Date(t * 1000).toLocaleString();
  }

  async function loadKeys() {
    const list = await api('GET', '/api/admin/v1/keys');
    keyId = list.length ? list[0].id : null;
    show($('key-row'), keyId !== null);
    show($('key-form'), keyId === null);
    if (keyId !== null) {
      $('key-row-name').textContent = list[0].name;
      $('key-row-times').textContent = 'made ' + new Date(list[0].created_at * 1000).toLocaleString() + ', ' + ago(list[0].last_used_at);
    }
    show($('key-error'), false);
  }

  on('key-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('key-error');
    show(err, false);
    try {
      const made = await api('POST', '/api/admin/v1/keys', { name: $('key-name').value });
      $('key-name').value = '';
      $('key-value').textContent = made.key; // the only time it is on the page
      $('key-dialog').showModal();
      await loadKeys();
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  function closeKeyDialog() {
    $('key-value').textContent = '';
    if ($('key-dialog').open) $('key-dialog').close();
  }
  on('key-close', 'click', closeKeyDialog);
  on('key-dialog', 'cancel', () => { $('key-value').textContent = ''; });
  on('key-copy', 'click', async () => {
    const text = $('key-value').textContent;
    try {
      await navigator.clipboard.writeText(text);
      $('key-copy').textContent = 'Copied';
    } catch (_) {
      // No clipboard here (plain http off localhost): select the text so Ctrl+C works.
      const range = document.createRange();
      range.selectNodeContents($('key-value'));
      const sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
      $('key-copy').textContent = 'Selected: press Ctrl+C';
    }
    setTimeout(() => { $('key-copy').textContent = 'Copy'; }, 3000);
  });

  on('key-revoke', 'click', async () => {
    const b = $('key-revoke');
    if (!revokeArmed) {
      b.textContent = 'Revoke? It stops working at once.';
      revokeArmed = setTimeout(() => { revokeArmed = null; b.textContent = 'Revoke'; }, 5000);
      return;
    }
    clearTimeout(revokeArmed);
    revokeArmed = null;
    b.textContent = 'Revoke';
    try {
      await api('DELETE', '/api/admin/v1/keys/' + encodeURIComponent(keyId));
      flash('The API key was revoked.');
      await loadKeys();
    } catch (ex) {
      $('key-error').textContent = ex.message;
      show($('key-error'), true);
    }
  });

  // ----------------------------------------------------------- network

  async function loadNetwork() {
    const n = await api('GET', '/api/admin/v1/network');
    $('n-tls').checked = n.tls_enabled;
    $('n-host').value = n.hostname;
    $('n-cidr').value = n.admin_cidr;
    $('n-allow').checked = !!n.allowlist_enabled;
    $('n-staging').checked = n.acme_staging;
    $('n-token').value = '';
    $('n-token').placeholder = n.has_token ? 'unchanged (write-only)' : 'write-only';
    $('n-clear').checked = false;
    show($('n-clear-row'), n.has_token);
    $('n-info').textContent = (n.tls_enabled && n.hostname ? 'The admin site is at https://' + n.hostname + '/admin. ' : '')
      + 'New pairing keys point at ' + n.public_url + '.';
    showProxy(n.proxy);
    show($('network-error'), false);
  }

  function showProxy(p) {
    const el = $('n-proxy');
    el.className = p.ok ? 'ok' : (p.at ? 'error' : 'hint');
    el.textContent = 'HTTPS front end: ' + p.message + (p.at ? ' (' + new Date(p.at * 1000).toLocaleTimeString() + ')' : '');
  }

  on('n-apply', 'click', async () => {
    try { showProxy(await api('POST', '/api/admin/v1/network/apply')); } catch (ex) { flash(ex.message); }
  });

  on('network-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('network-error');
    show(err, false);
    try {
      await api('PUT', '/api/admin/v1/network', {
        tls_enabled: $('n-tls').checked,
        hostname: $('n-host').value.trim(),
        admin_cidr: $('n-cidr').value.trim(),
        allowlist_enabled: $('n-allow').checked,
        acme_staging: $('n-staging').checked,
        cloudflare_token: $('n-token').value,
        clear_token: $('n-clear').checked,
      });
      await loadNetwork();
      flash('Network settings saved.');
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  // ---------------------------------------------------------- clients

  function when(ts) {
    return ts ? new Date(ts * 1000).toLocaleString() : 'never';
  }

  let userNames = {};
  async function loadUsers() {
    const users = await api('GET', '/api/admin/v1/users');
    userNames = Object.fromEntries(users.map((u) => [u.id, u.name]));
    const box = $('users');
    box.textContent = '';
    if (!users.length) {
      const p = document.createElement('p');
      p.className = 'hint';
      p.textContent = 'No users yet. Add one to pair an app for them.';
      box.appendChild(p);
    }
    // One row for each user, with the emergency stop for that person: Pause stops their replies (new and running) until
    // Resume. Everything is set as text.
    for (const u of users) {
      const row = document.createElement('div');
      row.className = 'sealed-row';
      row.dataset.user = u.id;
      const what = document.createElement('span');
      what.className = 'what';
      what.textContent = u.name + (u.paused ? ' · replies paused' : '');
      row.appendChild(what);
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'secondary';
      b.dataset.action = u.paused ? 'resume' : 'pause';
      b.textContent = u.paused ? 'Resume replies' : 'Pause replies';
      b.addEventListener('click', async () => {
        show($('user-error'), false);
        try {
          await api('POST', '/api/admin/v1/users/' + encodeURIComponent(u.id) + '/' + (u.paused ? 'resume' : 'pause'));
          await loadUsers();
        } catch (ex) {
          $('user-error').textContent = ex.message;
          show($('user-error'), true);
        }
      });
      row.appendChild(b);
      const del = document.createElement('button');
      del.type = 'button';
      del.className = 'secondary danger';
      del.dataset.action = 'delete';
      del.textContent = 'Delete…';
      del.addEventListener('click', () => askToDelete(row, u, del));
      row.appendChild(del);
      box.appendChild(row);
    }
    const sel = $('d-user');
    const keep = sel.value;
    sel.textContent = '';
    for (const u of users) {
      const o = document.createElement('option');
      o.value = u.id;
      o.textContent = u.name;
      sel.appendChild(o);
    }
    if (users.some((u) => u.id === keep)) sel.value = keep;
    $('pair-form').querySelector('button[type=submit]').disabled = users.length === 0;
  }

  // Deleting a person removes their personal space, their clients and their sign-ins for good (the audit records stay).
  // The name is typed to confirm, and the server wants a sign-in from the last 10 minutes.
  function askToDelete(row, u, opener) {
    opener.disabled = true;
    const box = document.createElement('div');
    box.className = 'confirm-delete';
    box.dataset.confirm = u.id;
    const warn = document.createElement('p');
    warn.className = 'hint';
    warn.textContent =
      'This deletes ' + u.name + "'s space, with every conversation in it, and signs out their apps. It cannot be undone. " +
      'Audit records stay. Type the name to confirm.';
    const input = document.createElement('input');
    input.type = 'text';
    input.autocomplete = 'off';
    input.setAttribute('aria-label', 'Type ' + u.name + ' to confirm');
    const go = document.createElement('button');
    go.type = 'button';
    go.className = 'danger';
    go.textContent = 'Delete ' + u.name;
    go.disabled = true;
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'secondary';
    cancel.textContent = 'Cancel';
    const err = document.createElement('p');
    err.className = 'error';
    err.setAttribute('role', 'alert');
    err.hidden = true;
    input.addEventListener('input', () => {
      go.disabled = input.value !== u.name;
    });
    cancel.addEventListener('click', () => {
      box.remove();
      opener.disabled = false;
    });
    go.addEventListener('click', async () => {
      if (input.value !== u.name) return;
      go.disabled = true;
      err.hidden = true;
      try {
        await api('DELETE', '/api/admin/v1/users/' + encodeURIComponent(u.id));
        await loadUsers();
        await loadClients();
      } catch (ex) {
        err.textContent = ex.message;
        err.hidden = false;
        go.disabled = input.value !== u.name;
      }
    });
    for (const el of [warn, input, go, cancel, err]) box.appendChild(el);
    row.appendChild(box);
    input.focus();
  }

  on('user-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('user-error');
    show(err, false);
    try {
      const r = await api('POST', '/api/admin/v1/users', { name: $('u-name').value.trim() });
      $('u-name').value = '';
      await loadUsers();
      $('d-user').value = r.id;
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  async function loadClients() {
    await loadUsers();
    const list = await api('GET', '/api/admin/v1/clients');
    const box = $('clients');
    box.textContent = '';
    if (!list.length) {
      const p = document.createElement('p');
      p.className = 'hint';
      p.textContent = 'No clients are paired yet.';
      box.appendChild(p);
    }
    for (const d of list) {
      const card = document.createElement('div');
      card.className = 'card';
      card.dataset.client = d.id;
      const head = document.createElement('div');
      head.className = 'row';
      const name = document.createElement('span');
      name.className = 'name';
      name.textContent = d.name;
      const badge = document.createElement('span');
      badge.className = 'badge';
      badge.textContent = d.revoked ? 'revoked' : 'active';
      const sp = document.createElement('span');
      sp.className = 'spacer';
      head.append(name, badge, sp);
      if (!d.revoked) {
        const revoke = button('Revoke', 'danger', async () => {
          if (!confirm('Revoke "' + d.name + '"? It will be signed out and must be paired again.')) return;
          try { await api('POST', '/api/admin/v1/clients/' + encodeURIComponent(d.id) + '/revoke'); loadClients(); } catch (ex) { flash(ex.message); }
        });
        revoke.dataset.action = 'revoke';
        head.appendChild(revoke);
      }
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = (userNames[d.user_id] || 'unknown user') + ' · paired ' + when(d.created_at) + ' · last seen ' + when(d.last_seen);
      card.append(head, meta);
      box.appendChild(card);
    }
  }

  on('pair-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('pair-error');
    show(err, false);
    try {
      const r = await api('POST', '/api/admin/v1/users/' + encodeURIComponent($('d-user').value) + '/pairing-keys', { name: $('d-name').value.trim() });
      $('pairing-string').value = r.pairing;
      show($('pairing-out'), true);
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  on('pairing-copy', 'click', async () => {
    const t = $('pairing-string');
    t.select();
    try { await navigator.clipboard.writeText(t.value); flash('Copied.'); } catch (_) { document.execCommand('copy'); flash('Copied.'); }
  });

  // --------------------------------------------------------- settings

  // "120 seconds = 2 minutes", "90 seconds = 1.5 minutes".
  function minutesText(secs) {
    if (!Number.isFinite(secs) || secs <= 0) return '';
    const m = secs / 60;
    const shown = Number.isInteger(m) ? String(m) : String(Math.round(m * 10) / 10);
    return secs + ' seconds = ' + shown + (shown === '1' ? ' minute' : ' minutes');
  }
  function showTimeoutMinutes() {
    $('s-gen-timeout-min').textContent = minutesText(parseInt($('s-gen-timeout').value, 10));
  }
  on('s-gen-timeout', 'input', showTimeoutMinutes);

  async function loadSettings() {
    const s = await api('GET', '/api/admin/v1/settings');
    $('s-retention').value = s.audit_retention_days == null ? 30 : s.audit_retention_days;
    $('s-unseal-idle').value = s.unseal_idle_minutes == null ? 15 : s.unseal_idle_minutes;
    $('s-unseal-total').value = s.unseal_total_minutes == null ? 60 : s.unseal_total_minutes;
    $('s-key-id').textContent = s.audit_key_id || 'none';
    $('s-kill-switch').checked = !!s.kill_switch;
    $('s-kill-aborts').checked = s.kill_switch_aborts_running !== false;
    $('s-gen-timeout').value = s.generation_timeout_secs == null ? 300 : s.generation_timeout_secs;
    showTimeoutMinutes();
    $('s-history-turns').value = s.history_turns == null ? 0 : s.history_turns;
    $('s-prompt').value = s.system_prompt;
    $('s-notice-on').checked = s.privacy_notice_enabled !== false;
    $('s-notice-text').value = s.privacy_notice_text || '';
    $('s-warning-on').checked = s.ai_warning_enabled !== false;
    $('s-warning-text').value = s.ai_warning_text || '';
    $('s-theme').value = s.display.theme;
    $('s-width').value = s.display.width;
    $('s-max-height').value = s.display.max_height;
    $('s-history-width').value = s.display.history_width;
    $('s-top').value = s.display.top_percent;
    $('s-hide-on-blur').checked = !!s.display.hide_on_blur;
    $('s-newest-first').checked = !!s.display.newest_first;
    show($('settings-error'), false);
    loadAlerts().catch(() => {});
  }

  // ---- How alerts are delivered (ADR 0055). The values are secrets: they are never shown, so the fields are emptied after a
  // save, and an empty token means "keep the one that is stored".
  const yesNo = (held) => (held ? 'yes' : 'no');
  async function loadAlerts() {
    const a = await api('GET', '/api/admin/v1/alerts');
    $('a-held-server').textContent = yesNo(a.ntfy.server);
    $('a-held-topic').textContent = yesNo(a.ntfy.topic);
    $('a-held-token').textContent = yesNo(a.ntfy.token);
    $('a-token-clear').checked = false;
    show($('a-error'), false);
    showAlertsBanner(a.banner);
  }
  function alertsError(ex) {
    $('a-error').textContent = ex.message;
    show($('a-error'), true);
  }
  on('alerts-form', 'submit', async (e) => {
    e.preventDefault();
    show($('a-error'), false);
    $('a-result').textContent = '';
    const ntfy = { server: $('a-server').value.trim(), topic: $('a-topic').value.trim() };
    if ($('a-token-clear').checked) ntfy.token = '';
    else if ($('a-token').value.trim()) ntfy.token = $('a-token').value.trim();
    try {
      await api('PUT', '/api/admin/v1/alerts', { ntfy });
      for (const id of ['a-server', 'a-topic', 'a-token']) $(id).value = '';
      await loadAlerts();
      flash('Alert settings saved. Send a test alert to check that it arrives.');
    } catch (ex) { alertsError(ex); }
  });
  on('a-test', 'click', async () => {
    show($('a-error'), false);
    $('a-result').textContent = 'Sending a test alert...';
    try {
      const r = await api('POST', '/api/admin/v1/alerts/test');
      const why = { unreachable: 'the server did not answer', refused: 'the server refused it', unauthorized: 'the token was not accepted', timeout: 'the server took too long' };
      $('a-result').textContent = r.results.length
        ? r.results.map((c) => c.channel + ': ' + (c.ok ? 'delivered, check your phone' : 'failed, ' + (why[c.problem] || 'unknown problem'))).join('. ')
        : 'No delivery channel is set up, so nothing was sent.';
    } catch (ex) { $('a-result').textContent = ''; alertsError(ex); }
  });
  on('a-remove', 'click', async () => {
    if (!confirm('Stop delivering alerts? Nothing will tell you when something needs attention until you set it up again.')) return;
    show($('a-error'), false);
    try {
      await api('PUT', '/api/admin/v1/alerts', { ntfy: null });
      $('a-result').textContent = '';
      await loadAlerts();
      flash('Alert delivery removed.');
    } catch (ex) { alertsError(ex); }
  });

  on('settings-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('settings-error');
    show(err, false);
    try {
      await api('PUT', '/api/admin/v1/settings', {
        system_prompt: $('s-prompt').value,
        generation_timeout_secs: parseInt($('s-gen-timeout').value, 10),
        history_turns: parseInt($('s-history-turns').value, 10),
        audit_retention_days: parseInt($('s-retention').value, 10),
        unseal_idle_minutes: parseInt($('s-unseal-idle').value, 10),
        unseal_total_minutes: parseInt($('s-unseal-total').value, 10),
        kill_switch: $('s-kill-switch').checked,
        kill_switch_aborts_running: $('s-kill-aborts').checked,
        privacy_notice_enabled: $('s-notice-on').checked,
        privacy_notice_text: $('s-notice-text').value,
        ai_warning_enabled: $('s-warning-on').checked,
        ai_warning_text: $('s-warning-text').value,
        display: {
          theme: $('s-theme').value,
          width: parseInt($('s-width').value, 10),
          max_height: parseInt($('s-max-height').value, 10),
          history_width: parseInt($('s-history-width').value, 10),
          top_percent: parseFloat($('s-top').value),
          hide_on_blur: $('s-hide-on-blur').checked,
          newest_first: $('s-newest-first').checked,
        },
      });
      flash('Settings saved. Clients pick them up on their next message or when their window opens.');
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  // The audit key: switch to a new passphrase. The old key is kept, only for reading what it sealed.
  on('rotate-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('rotate-error');
    show(err, false);
    const fail = (m) => { err.textContent = m; show(err, true); };
    const cur = $('r-current').value;
    const next = $('r-new').value;
    if (next !== $('r-new2').value) return fail('The two new passphrases do not match.');
    if ([...next].length < 20) return fail('The new passphrase must be at least 20 characters.');
    try {
      const r = await api('POST', '/api/admin/v1/audit/rotate-key', { current: cur, new: next, confirm: $('r-new2').value });
      for (const id of ['r-current', 'r-new', 'r-new2']) $(id).value = '';
      $('s-key-id').textContent = r.key_id;
      flash('The audit key was rotated. New key id ' + r.key_id + '. Content sealed before still needs the old passphrase.');
    } catch (ex) {
      $('r-current').value = '';
      fail(ex.message);
    }
  });

  // ------------------------------------------------------------- boot

  (async () => {
    route();
  })();
})();
