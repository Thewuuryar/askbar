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
    if (res.status === 401 && path !== '/admin/login') { showLogin(); throw new Error('Signed out'); }
    let data = null;
    const text = await res.text();
    if (text) { try { data = JSON.parse(text); } catch (_) { data = null; } }
    if (!res.ok) {
      const err = new Error((data && data.error) || ('Request failed (' + res.status + ')'));
      err.status = res.status;
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
    { id: 'devices', label: 'Devices', load: loadDevices },
    { id: 'settings', label: 'Settings', load: loadSettings },
    { id: 'security', label: 'Security' },
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

  // First run: choose a password, then create the master key. Returns when nothing is left to set up.
  let pendingKey = null; // the master key, held only until the user confirms they saved it

  function showSetup(status) {
    hideAll();
    show($('setup'), true);
    show($('setup-password-form'), !status.password_set);
    show($('setup-key'), status.password_set && !status.key_set);
    show($('setup-saved'), false);
    if (!status.password_set) $('setup-password').focus();
  }

  async function route() {
    let status;
    try { status = await api('GET', '/admin/api/setup-status'); } catch (_) { status = { password_set: true, key_set: true }; }
    if (!status.password_set) return showSetup(status);
    let signedIn = true;
    try { await api('GET', '/admin/session'); } catch (_) { signedIn = false; }
    if (!signedIn) return showLogin();
    if (pendingKey) return showSavedKey();
    if (!status.key_set) return showSetup(status);
    showApp();
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
      await api('POST', '/admin/api/setup/password', { password: $('setup-password').value });
      $('setup-password').value = '';
      $('setup-password2').value = '';
      route();
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  async function createKey(body) {
    const err = $('setup-key-error');
    show(err, false);
    try {
      const r = await api('POST', '/admin/api/setup/master-key', body);
      pendingKey = r.key;
      $('setup-key-input').value = '';
      route();
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  }

  function showSavedKey() {
    hideAll();
    show($('setup'), true);
    for (const id of ['setup-password-form', 'setup-key']) show($(id), false);
    show($('setup-saved'), true);
    $('saved-key').value = pendingKey;
    $('saved-confirm').checked = false;
    $('saved-continue').disabled = true;
    $('saved-key').select();
  }

  $('setup-key-button').addEventListener('click', () => createKey({}));
  $('setup-key-existing').addEventListener('click', () => { show($('setup-key-form'), true); $('setup-key-input').focus(); });
  $('setup-key-form').addEventListener('submit', (e) => { e.preventDefault(); createKey({ key: $('setup-key-input').value }); });

  $('saved-copy').addEventListener('click', async () => {
    const field = $('saved-key');
    field.select();
    try { await navigator.clipboard.writeText(field.value); } catch (_) { document.execCommand('copy'); }
    $('saved-copy').textContent = 'Copied';
    setTimeout(() => { $('saved-copy').textContent = 'Copy to clipboard'; }, 1500);
  });
  $('saved-confirm').addEventListener('change', () => { $('saved-continue').disabled = !$('saved-confirm').checked; });
  $('saved-continue').addEventListener('click', () => {
    pendingKey = null;
    $('saved-key').value = '';
    route();
  });

  function showLogin() {
    hideAll();
    show($('login'), true);
    $('password').value = '';
    $('password').focus();
  }

  function showApp() {
    hideAll();
    show($('app'), true);
    renderNav();
    openTab(tabs[0].id);
  }

  $('login-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const err = $('login-error');
    show(err, false);
    try {
      await api('POST', '/admin/login', { password: $('password').value });
      route();
    } catch (ex) {
      err.textContent = ex.status === 429 ? 'Too many attempts. Try again later.'
        : ex.status === 503 ? 'No admin password is set yet. Reload this page to set one up.'
        : 'Wrong password.';
      show(err, true);
    }
  });

  $('logout').addEventListener('click', async () => {
    try { await api('POST', '/admin/logout'); } catch (_) { /* already signed out */ }
    route();
  });

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
        const r = await api('POST', '/admin/api/providers/' + encodeURIComponent(p.id) + '/test');
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
      try { await api('DELETE', '/admin/api/providers/' + encodeURIComponent(p.id)); loadProviders(); } catch (ex) { flash(ex.message); }
    });
    del.dataset.action = 'delete';
    head.append(test, edit, del);

    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = p.model + (p.base_url ? ' · ' + p.base_url : '');
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
    const list = await api('GET', '/admin/api/providers');
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

  function openForm(p, preset) {
    editing = p ? p.id : null;
    const v = p || preset || { name: '', type: 'openai', base_url: '', model: '' };
    $('provider-form-title').textContent = p ? 'Edit ' + p.name : 'Add provider';
    $('p-name').value = v.name || '';
    $('p-type').value = v.type || 'openai';
    $('p-base-url').value = v.base_url || '';
    $('p-model').value = v.model || '';
    $('p-key').value = ''; // never prefilled
    $('p-key').placeholder = p && p.has_key ? 'unchanged (write-only)' : 'write-only';
    $('p-max-tokens').value = v.max_tokens || '';
    $('p-temperature').value = v.temperature == null ? '' : v.temperature;
    show($('provider-error'), false);
    show($('provider-form'), true);
    $('p-name').focus();
  }

  function closeForm() {
    editing = null;
    $('p-key').value = '';
    show($('provider-form'), false);
  }

  $('add-provider').addEventListener('click', () => openForm(null));
  // CLIProxyAPI runs next to the server in the compose network (profile `cpa`).
  $('add-cpa').addEventListener('click', () => openForm(null, { name: 'cpa', type: 'openai', base_url: 'http://cpa:8317/v1', model: '' }));
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
    };
    try {
      if (editing) await api('PUT', '/admin/api/providers/' + encodeURIComponent(editing), body);
      else await api('POST', '/admin/api/providers', body);
      closeForm();
      loadProviders();
    } catch (ex) {
      $('provider-error').textContent = ex.message;
      show($('provider-error'), true);
    }
  });

  // --------------------------------------------------------- security

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
      await api('POST', '/admin/api/password', { current: $('pw-current').value, new: $('pw-new').value });
      for (const id of ['pw-current', 'pw-new', 'pw-new2']) $(id).value = '';
      flash('Password changed. Other browsers have been signed out.');
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  // ---------------------------------------------------------- devices

  function when(ts) {
    return ts ? new Date(ts * 1000).toLocaleString() : 'never';
  }

  async function loadDevices() {
    const list = await api('GET', '/admin/api/devices');
    const box = $('devices');
    box.textContent = '';
    if (!list.length) {
      const p = document.createElement('p');
      p.className = 'hint';
      p.textContent = 'No devices are paired yet.';
      box.appendChild(p);
    }
    for (const d of list) {
      const card = document.createElement('div');
      card.className = 'card';
      card.dataset.device = d.id;
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
          try { await api('POST', '/admin/api/devices/' + encodeURIComponent(d.id) + '/revoke'); loadDevices(); } catch (ex) { flash(ex.message); }
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
      const r = await api('POST', '/admin/api/pair', { name: $('d-name').value.trim() });
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

  async function loadSettings() {
    const s = await api('GET', '/admin/api/settings');
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
      await api('PUT', '/admin/api/settings', {
        system_prompt: $('s-prompt').value,
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

  // ------------------------------------------------------------- boot

  (async () => {
    route();
  })();
})();
