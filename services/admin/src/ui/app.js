// Askbar admin. Plain JavaScript, no build step. Everything user-supplied goes in via textContent.
(() => {
  'use strict';

  const $ = (id) => document.getElementById(id);
  const show = (node, on) => { node.hidden = !on; };

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

  // ------------------------------------------------------------- tabs

  const tabs = [
    { id: 'providers', label: 'Providers', load: loadProviders },
    { id: 'services', label: 'Services', load: loadServices },
    { id: 'clients', label: 'Clients', load: loadClients },
    { id: 'settings', label: 'Settings', load: loadSettings },
    { id: 'network', label: 'Network', load: loadNetwork },
    { id: 'security', label: 'Security', load: loadKeys },
    { id: 'backup', label: 'Backup' },
  ];
  function registerTab(tab) { tabs.push(tab); renderNav(); }
  window.askbarAdmin = { api, flash, registerTab, show, $ };

  function openTab(id) {
    for (const t of tabs) {
      const sec = document.querySelector('section[data-tab="' + t.id + '"]');
      if (sec) show(sec, t.id === id);
    }
    for (const b of $('nav').children) b.setAttribute('aria-current', b.dataset.tab === id ? 'page' : 'false');
    flash('');
    const t = tabs.find((x) => x.id === id);
    if (t && t.load) t.load().catch((e) => flash(e.message));
  }

  function renderNav() {
    const nav = $('nav');
    nav.textContent = '';
    for (const t of tabs) {
      const b = document.createElement('button');
      b.type = 'button';
      b.dataset.tab = t.id;
      b.textContent = t.label;
      b.addEventListener('click', () => openTab(t.id));
      nav.appendChild(b);
    }
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

  async function route(afterTab) {
    let signedIn = true;
    try { await api('GET', '/api/admin/v1/session'); } catch (_) { signedIn = false; }
    if (!signedIn) return showLogin();
    let status;
    try { status = await api('GET', '/api/admin/v1/setup'); } catch (_) { status = {}; }
    if (status.password_change_required) return showPasswordChange();
    // The audit passphrase is part of setup: without a key the server records nothing it may keep,
    // and refuses to start replies. An install from before audit keys is asked on its next visit.
    if (status.audit_key_set === false) return showAuditSetup();
    showApp(afterTab);
  }

  function showAuditSetup() {
    hideAll();
    show($('setup'), true);
    show($('setup-password-form'), false);
    show($('setup-audit-done'), false);
    show($('setup-audit-form'), true);
    $('setup-audit').focus();
  }

  $('setup-audit-form').addEventListener('submit', async (e) => {
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
  $('setup-audit-continue').addEventListener('click', () => route());
  for (const a of document.querySelectorAll('#setup-next a')) {
    a.addEventListener('click', (e) => { e.preventDefault(); route(a.dataset.tab); });
  }

  $('setup-password-form').addEventListener('submit', async (e) => {
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

  // The "server offline" banner: ask the admin how its link to the server is, now and then.
  let statusTimer = null;
  async function checkServer() {
    try {
      const st = await api('GET', '/api/admin/v1/status');
      show($('offline'), !st.server_online);
    } catch (_) { /* signed out, or the admin itself is away */ }
  }

  function showApp(tab) {
    hideAll();
    show($('app'), true);
    renderNav();
    openTab(tabs.some((t) => t.id === tab) ? tab : tabs[0].id);
    checkServer();
    if (!statusTimer) statusTimer = setInterval(() => { if (!$('app').hidden) checkServer(); }, 5000);
  }

  $('login-form').addEventListener('submit', async (e) => {
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

  $('logout').addEventListener('click', async () => {
    try { await api('DELETE', '/api/admin/v1/session'); } catch (_) { /* already signed out */ }
    route();
  });

  // -------------------------------------------------------- services

  const SERVICE_INFO = {
    server: { title: 'Server', what: 'the Askbar server (chats, history, provider calls)', warn: 'Replies being written end as interrupted, and apps say "server offline" until it is back (under a minute).' },
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
  $('cpa-connect-anthropic').addEventListener('click', () => connect('anthropic'));
  $('cpa-connect-openai').addEventListener('click', () => connect('openai'));
  $('cpa-cancel').addEventListener('click', closeLogin);

  $('cpa-finish').addEventListener('click', async () => {
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
    key.textContent = p.has_key ? 'key set' : 'no key';
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
        result.textContent = r.ok ? 'Success: ' + r.message : 'Failed: ' + r.message;
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
  }

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

  $('p-model-list').addEventListener('change', () => {
    if ($('p-model-list').value === MANUAL) { showModelInput(); $('p-model').focus(); return; }
    $('p-model').value = $('p-model-list').value;
  });
  $('p-model-refresh').addEventListener('click', loadModels);
  for (const id of ['p-type', 'p-base-url', 'p-key']) $(id).addEventListener('change', loadModels);

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

  $('add-provider').addEventListener('click', () => openForm(null));
  $('provider-cancel').addEventListener('click', closeForm);

  $('provider-form').addEventListener('submit', async (e) => {
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

  $('password-form').addEventListener('submit', async (e) => {
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

  $('key-form').addEventListener('submit', async (e) => {
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
  $('key-close').addEventListener('click', closeKeyDialog);
  $('key-dialog').addEventListener('cancel', () => { $('key-value').textContent = ''; });
  $('key-copy').addEventListener('click', async () => {
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

  $('key-revoke').addEventListener('click', async () => {
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
      + 'New pairing strings point at ' + n.public_url + '.';
    showProxy(n.proxy);
    show($('network-error'), false);
  }

  function showProxy(p) {
    const el = $('n-proxy');
    el.className = p.ok ? 'ok' : (p.at ? 'error' : 'hint');
    el.textContent = 'HTTPS front end: ' + p.message + (p.at ? ' (' + new Date(p.at * 1000).toLocaleTimeString() + ')' : '');
  }

  $('n-apply').addEventListener('click', async () => {
    try { showProxy(await api('POST', '/api/admin/v1/network/apply')); } catch (ex) { flash(ex.message); }
  });

  $('network-form').addEventListener('submit', async (e) => {
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

  async function loadClients() {
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
      meta.textContent = 'Paired ' + when(d.created_at) + ' · last seen ' + when(d.last_seen);
      card.append(head, meta);
      box.appendChild(card);
    }
  }

  $('pair-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const err = $('pair-error');
    show(err, false);
    try {
      const r = await api('POST', '/api/admin/v1/pairings', { name: $('d-name').value.trim() });
      $('pairing-string').value = r.pairing;
      show($('pairing-out'), true);
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  $('pairing-copy').addEventListener('click', async () => {
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
  $('s-gen-timeout').addEventListener('input', showTimeoutMinutes);

  async function loadSettings() {
    const s = await api('GET', '/api/admin/v1/settings');
    $('s-retention').value = s.audit_retention_days == null ? 30 : s.audit_retention_days;
    $('s-key-id').textContent = s.audit_key_id || 'none';
    $('s-kill-switch').checked = !!s.kill_switch;
    $('s-kill-aborts').checked = s.kill_switch_aborts_running !== false;
    $('s-gen-timeout').value = s.generation_timeout_secs == null ? 300 : s.generation_timeout_secs;
    showTimeoutMinutes();
    $('s-prompt').value = s.system_prompt;
    $('s-theme').value = s.display.theme;
    $('s-width').value = s.display.width;
    $('s-max-height').value = s.display.max_height;
    $('s-history-width').value = s.display.history_width;
    $('s-top').value = s.display.top_percent;
    $('s-hide-on-blur').checked = !!s.display.hide_on_blur;
    show($('settings-error'), false);
  }

  $('settings-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const err = $('settings-error');
    show(err, false);
    try {
      await api('PUT', '/api/admin/v1/settings', {
        system_prompt: $('s-prompt').value,
        generation_timeout_secs: parseInt($('s-gen-timeout').value, 10),
        audit_retention_days: parseInt($('s-retention').value, 10),
        kill_switch: $('s-kill-switch').checked,
        kill_switch_aborts_running: $('s-kill-aborts').checked,
        display: {
          theme: $('s-theme').value,
          width: parseInt($('s-width').value, 10),
          max_height: parseInt($('s-max-height').value, 10),
          history_width: parseInt($('s-history-width').value, 10),
          top_percent: parseFloat($('s-top').value),
          hide_on_blur: $('s-hide-on-blur').checked,
        },
      });
      flash('Settings saved. Clients pick them up on their next message or when their window opens.');
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  // The audit key: switch to a new passphrase. The old key is kept, only for reading what it sealed.
  $('rotate-form').addEventListener('submit', async (e) => {
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
