// Askbar admin. Plain JavaScript, no build step. Everything user-supplied goes in via textContent.
(() => {
  'use strict';

  const $ = (id) => document.getElementById(id);
  const show = (node, visible) => { node.hidden = !visible; };
  // Each page holds only its own section (plan step 1.16), so wiring for another page's elements is skipped.
  const on = (id, event, fn) => { const el = $(id); if (el) el.addEventListener(event, fn); };
  // The server's limits and intervals (ui.constants, from components.js). Set once at boot, before anything is shown.
  const { ui } = window;
  const MS = ui.MS;
  let K = null;

  // A live list reads with `{ etag: true }`: the answer's ETag is kept, sent back as If-None-Match, and a 304 hands back
  // the data kept with it. (The admin API sends no ETag on its lists today, so every answer is a full 200 and ui.live
  // compares the bodies instead; this is ready for the day it does.) A read is never counted as the person's activity:
  // the session has a fixed expiry that no request moves (auth.rs), and only changes are recorded (actions.rs).
  const etags = new Map();
  async function api(method, path, body, opts = {}) {
    const known = opts.etag ? etags.get(path) : null;
    const res = await fetch(path, {
      method,
      credentials: 'same-origin',
      headers: { 'X-Askbar-Admin': '1', ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}), ...(known ? { 'If-None-Match': known.tag } : {}) },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    if (res.status === 304 && known) return known.data;
    if (res.status === 401 && !(method === 'POST' && path === '/api/admin/v1/session')) { showLogin(); throw new Error('Signed out'); }
    let data = null;
    const text = await res.text();
    if (text) { try { data = JSON.parse(text); } catch (_) { data = null; } }
    if (res.status === 502) ui.banner('offline', 'error', OFFLINE_TEXT);
    if (opts.etag && res.ok && res.headers.get('ETag')) etags.set(path, { tag: res.headers.get('ETag'), data });
    if (!res.ok) {
      // A problem+json body: `detail` is the sentence for a person, `error` the stable code.
      const err = new Error((data && (data.detail || data.error)) || ('Request failed (' + res.status + ')'));
      err.status = res.status;
      err.code = (data && data.error) || null;
      throw err;
    }
    return data;
  }

  // Success goes to a toast; an error stays on the page (rule 32): inline next to the action when the page has a
  // line for it, else the one page-level line under the status bar. '' clears it.
  const OFFLINE_TEXT = "Server offline. The admin can't reach the Askbar server, so changes, tests, clients and backups won't work until it is back. Nothing you change now is lost or half-saved.";
  function pageError(text) {
    const line = $('page-error');
    line.textContent = text;
    show(line, !!text);
  }
  function inlineError(id, text) {
    const line = $(id);
    if (!line) return pageError(text);
    line.textContent = text;
    show(line, !!text);
  }

  // ------------------------------------------------------------- pages

  // One admin page per URL (plan step 1.16): the nav is plain links, and this script runs only the
  // current page's loader. A page with no entry here needs none; audit.js registers the Audit page's.
  const page = document.body.dataset.page;
  const tabs = [
    { id: 'home', load: loadHome },
    { id: 'providers', load: loadProviders },
    { id: 'services', load: loadServices },
    { id: 'clients', load: loadClients },
    { id: 'general', load: loadSettings },
    { id: 'network', load: loadNetwork },
    { id: 'security', load: loadKeys },
    { id: 'backup', load: loadBackup },
    { id: 'notifications', load: loadAlerts },
    { id: 'restore', load: loadRestore },
  ];
  let appShown = false;
  function registerTab(tab) {
    tabs.push(tab);
    if (appShown && tab.id === page) loadPage();
  }
  window.askbarAdmin = { api, pageError, registerTab, show, $, openSession, closeSession };

  function loadPage() {
    pageError('');
    const t = tabs.find((x) => x.id === page);
    if (t && t.load) t.load().catch((e) => pageError(e.message));
  }

  // ------------------------------------------------------------ login

  function hideAll() {
    for (const id of ['app', 'login', 'setup']) show($(id), false);
  }

  // First run (ADR 0007): signed in with the published password, change it before anything else.
  function showPasswordChange() {
    hideAll();
    show($('setup'), true);
    for (const id of ['setup-audit-form', 'setup-audit-done', 'setup-backup-form', 'setup-backup-done']) show($(id), false);
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
    // The Restore page is the way out of the setup screens: a restore is for an install nothing has been set up in.
    if (page === 'restore' && (status.audit_key_set === false || status.backup_key_set === false)) return showApp();
    if (status.audit_key_set === false) return showAuditSetup();
    // The backup passphrase is part of setup too (ADR 0013): without a key no backup can be encrypted. An install from before the
    // backup key is asked on its next visit.
    if (status.backup_key_set === false) return showBackupSetup();
    showApp();
  }

  function showAuditSetup() {
    hideAll();
    show($('setup'), true);
    for (const id of ['setup-password-form', 'setup-audit-done', 'setup-backup-form', 'setup-backup-done']) show($(id), false);
    show($('setup-audit-form'), true);
    $('setup-audit').focus();
  }

  function showBackupSetup() {
    hideAll();
    show($('setup'), true);
    for (const id of ['setup-password-form', 'setup-audit-form', 'setup-audit-done', 'setup-backup-done']) show($(id), false);
    show($('setup-backup-signout'), false);
    show($('setup-backup-form'), true);
    $('setup-backup').focus();
  }

  // The backup passphrase (ADR 0013, D22): the same rules as the audit one, and it needs a sign-in within the last 10 minutes, so a
  // slow first run is told to sign in again. The passphrase fields are emptied the moment it is sent.
  on('setup-backup-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('setup-backup-error');
    show(err, false);
    const fail = (m) => { err.textContent = m; show(err, true); };
    const pass = $('setup-backup').value;
    if (pass !== $('setup-backup2').value) return fail('The two passphrases do not match.');
    if ([...pass].length < K.MIN_PASSPHRASE_CHARS) return fail('The passphrase must be at least ' + K.MIN_PASSPHRASE_CHARS + ' characters.');
    if (!$('setup-backup-stored').checked) return fail('Confirm that you stored the passphrase: it cannot be recovered.');
    try {
      const r = await api('PUT', '/api/admin/v1/setup/backup-key', { passphrase: pass, confirm: $('setup-backup2').value });
      $('setup-backup').value = '';
      $('setup-backup2').value = '';
      $('setup-backup-stored').checked = false;
      $('setup-backup-fingerprint').textContent = r.fingerprint;
      show($('setup-backup-form'), false);
      show($('setup-backup-done'), true);
    } catch (ex) {
      fail(ex.message);
      // A sign-in that is too old needs signing in again; the setup screen has no other way to sign out.
      show($('setup-backup-signout'), ex.status === 403);
    }
  });
  on('setup-backup-signout', 'click', async () => {
    try { await api('DELETE', '/api/admin/v1/session'); } catch (_) { /* already signed out */ }
    route();
  });
  on('setup-backup-continue', 'click', () => route());

  on('setup-audit-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('setup-audit-error');
    show(err, false);
    const fail = (m) => { err.textContent = m; show(err, true); };
    const pass = $('setup-audit').value;
    if (pass !== $('setup-audit2').value) return fail('The two passphrases do not match.');
    if ([...pass].length < K.MIN_PASSPHRASE_CHARS) return fail('The passphrase must be at least ' + K.MIN_PASSPHRASE_CHARS + ' characters.');
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

  // Sealed content (ADR 0011). The one place the session is opened and closed: the status bar's seal toggle and the
  // Audit page's Content view both call these. The server keeps the time (idle and total seconds left, from the status);
  // the toggle counts down between status polls. Opening needs a sign-in no older than FRESH_SIGNIN_SECS: the server
  // refuses an older one with a sentence that says so, and the box shows it inline. Everything is set as text.
  const sealed = { open: false, left: 0 };
  function openSession(passphrase) { return api('POST', '/api/admin/v1/audit/session', { passphrase }); }
  async function closeSession() {
    try { await api('DELETE', '/api/admin/v1/audit/session'); } catch (_) { /* signed out */ }
  }
  function minutes(secs) { return Math.max(1, Math.ceil(secs / 60)); }
  const minuteText = (secs) => minutes(secs) + ' min';

  // The toggle: a word and a symbol for the state (rule 37) and one button with its own name. Sealed: "Unseal" opens
  // the box. Unsealed: the time left and "Seal", which closes the session at once.
  function renderSealToggle() {
    const status = $('sb-seal-status');
    const button = $('seal-button');
    if (!status || !button) return;
    status.className = 'status status-' + (sealed.open ? 'warning' : 'ok');
    status.querySelector('.status-word').textContent = sealed.open ? 'Unsealed · ' + minuteText(sealed.left) + ' left' : 'Sealed';
    button.hidden = false;
    button.textContent = sealed.open ? 'Seal' : 'Unseal';
    button.setAttribute('aria-label', sealed.open ? 'Seal content now' : 'Unseal content');
    button.setAttribute('aria-haspopup', sealed.open ? 'false' : 'dialog');
  }
  async function sealNow() {
    await closeSession();
    checkServer();
    if (window.askbarAudit) window.askbarAudit.loadSession();
    ui.announce('Sealed content is sealed.');
  }
  async function openUnsealBox() {
    const form = $('unseal-form');
    if (!form) return;
    $('unseal-pass').value = '';
    ui.fieldError('unseal-pass', '');
    $('unseal-fresh').textContent = 'Your sign-in must be less than ' + Math.round(K.FRESH_SIGNIN_SECS / 60) + ' minutes old; if it is older, sign in again first.';
    $('unseal-limits').textContent = 'Opening sends an alert, and every item you view is recorded.';
    ui.openBox('unseal-box');
    $('unseal-pass').focus();
    try { // the limits are the server's settings; the box reads them, it does not copy them
      const r = await api('GET', '/api/admin/v1/audit/session');
      $('unseal-limits').textContent = 'Seals itself after ' + minutes(r.idle_limit_secs) + ' minutes idle or ' + minutes(r.total_limit_secs) + ' in all. Opening sends an alert, and every item you view is recorded.';
    } catch (_) { /* the box works without it */ }
  }
  on('seal-button', 'click', () => { if (sealed.open) sealNow(); else openUnsealBox(); });
  on('unseal-cancel', 'click', () => ui.closeBox('unseal-box'));
  on('unseal-form', 'submit', async (e) => {
    e.preventDefault();
    const field = $('unseal-pass');
    const passphrase = field.value;
    field.value = ''; // gone from the field the moment it is sent, whatever the answer
    ui.fieldError('unseal-pass', '');
    if (!passphrase) { ui.fieldError('unseal-pass', 'Enter the audit passphrase.').focus(); return; }
    const done = ui.busy($('unseal-go'), 'Unsealing…');
    try {
      await openSession(passphrase);
    } catch (ex) {
      done();
      if (ex.message === 'Signed out') return;
      ui.fieldError('unseal-pass', ex.message).focus();
      return;
    }
    done();
    ui.closeBox('unseal-box');
    ui.announce('Sealed content is open.');
    await checkServer();
    if (window.askbarAudit) { window.askbarAudit.loadSession(); window.askbarAudit.loadAccess(); }
  });
  setInterval(() => {
    if (!sealed.open) return;
    sealed.left = Math.max(0, sealed.left - 1);
    renderSealToggle();
  }, ui.MS);

  // The status poll's part: the toggle, and one plain banner while it is open (no button: sealing lives in the toggle).
  // The day's alerts (an unseal, a change of the timeouts) are not banners: they are history, listed in the bell's Info
  // group (rule 39: page-wide banners are only for conditions that affect the whole admin).
  function showSealed(s) {
    const open = !!s.open;
    sealed.open = open;
    sealed.left = open ? Math.min(s.idle_left_secs || 0, s.total_left_secs || 0) : 0;
    renderSealToggle();
    const mins = minutes(sealed.left);
    ui.banner('unsealed', 'warning', open ? 'Sealed content is open for about ' + mins + ' more minute' + (mins === 1 ? '' : 's') + ' (less if it is not used).' : null, { live: true });
    if (window.askbarAudit && window.askbarAudit.sessionFromStatus) window.askbarAudit.sessionFromStatus(s);
  }

  // Alert delivery (ADR 0055): a banner when no channel is set up, when delivery has not worked for half an hour, or when the
  // alerter does not answer. The sentences are the admin's own, set as text.
  const ALERTS_BANNER = {
    not_set_up: 'Alerts are not set up, so nothing will tell you when something needs attention. Set up ntfy under Settings, then send a test alert.',
    failing: 'Alert delivery has not worked for half an hour. Check the ntfy settings under Settings and send a test alert.',
    unreachable: 'The alerter does not answer, so no alert is delivered. Look at it on the Services page.',
  };
  function showAlertsBanner(state) {
    ui.banner('alerts', 'error', ALERTS_BANNER[state] || null);
  }

  // The commands are shown as code (built as elements, never as HTML).
  const REJECTED_PARTS = [
    "The server rejected this admin's token. Rotate it on the Docker host with ",
    { code: 'docker compose run --rm bootstrap rotate admin-server' },
    ', then restart both: ',
    { code: 'docker compose restart server admin' },
    ". Until then the admin can't change anything on the server. The banner goes away when the server accepts the token again.",
  ];

  // The alerts bell (plan step 3.4, rule 38). There is no alerts API: the list is built here from the status response, one
  // row per condition below. Severity: critical = the whole admin cannot do its job or the log cannot be trusted (server
  // offline, token rejected, audit chain broken); warning = something to fix soon (alert delivery not set up, failing or
  // unreachable; sealed content open); info = the day's recent `alerts` entries. `href` is where it is fixed. Only a
  // critical row pops up. Delivery is fixed in Notifications, where ntfy and email are set.
  const BELL = {
    offline: { severity: 'critical', fix: 'Services', href: '/admin/services/', text: "The server is offline. Changes, tests, clients and backups won't work until it is back." },
    rejected: { severity: 'critical', fix: 'Services', href: '/admin/services/', text: "The server rejected this admin's token. Changes can't be saved until it is replaced." },
    chain: { severity: 'critical', fix: 'Audit', href: '/admin/audit/calls/', text: 'The audit records no longer match what the admin stored.' },
    not_set_up: { severity: 'warning', fix: 'Settings', href: '/admin/notifications/#alerts-form', text: 'Alerts are not set up, so nothing will tell you when something needs attention.' },
    failing: { severity: 'warning', fix: 'Settings', href: '/admin/notifications/#alerts-form', text: 'Alert delivery has not worked for half an hour.' },
    unreachable: { severity: 'warning', fix: 'Services', href: '/admin/services/', text: 'The alerter does not answer, so no alert is delivered.' },
    sealed: { severity: 'warning', fix: 'Audit / Sealed access', href: '/admin/audit/sealed/', text: 'Sealed content is open.' },
  };
  // Info rows come from `alerts` entries by kind (ADR 0055 names); a kind not listed goes to Admin actions. The two kinds
  // that are conditions above are left out, so nothing is counted twice.
  const BELL_KIND_FIX = {
    unsealed: { fix: 'Audit / Sealed access', href: '/admin/audit/sealed/' },
    audit_disk_low: { fix: 'Services', href: '/admin/services/' },
    audit_disk_critical: { fix: 'Services', href: '/admin/services/' },
    component_silent: { fix: 'Services', href: '/admin/services/' },
    caddy_silent: { fix: 'Services', href: '/admin/services/' },
    restart_loop: { fix: 'Services', href: '/admin/services/' },
    backup_failed: { fix: 'Backup', href: '/admin/backup/' },
    backup_missed: { fix: 'Backup', href: '/admin/backup/' },
    restore_performed: { fix: 'Backup', href: '/admin/backup/' },
  };
  const BELL_KIND_DEFAULT = { fix: 'Admin actions', href: '/admin/audit/admin/' };
  const BELL_SKIP_KINDS = ['chain_mismatch', 'rejected_token'];

  // What the badge counts, and what Home's Needs attention lists first: critical plus warning, never info. One function, so
  // the two cannot disagree.
  const urgentOf = (items) => items.filter((i) => i.severity !== 'info');

  // Dismissed Info alerts (Alerts page): there is no server API for it, so the keys (kind + at) are kept in this browser's
  // localStorage. A blocked store only means a dismissal lasts until the page is reloaded.
  const DISMISSED_KEY = 'askbar-admin.dismissed-alerts';
  const DISMISSED_MAX = 200;
  const dismissed = [];
  try { dismissed.push(...JSON.parse(localStorage.getItem(DISMISSED_KEY) || '[]')); } catch (_) { /* no storage */ }
  function dismissAlert(key) {
    if (dismissed.includes(key)) return;
    dismissed.push(key);
    if (dismissed.length > DISMISSED_MAX) dismissed.splice(0, dismissed.length - DISMISSED_MAX);
    try { localStorage.setItem(DISMISSED_KEY, JSON.stringify(dismissed)); } catch (_) { /* no storage */ }
  }

  // The one builder of alert rows: the bell, Home's Needs attention and the Alerts page all read what it returns, so their
  // counts cannot differ. A dismissed Info alert is left out everywhere.
  function bellItems(st) {
    const items = [];
    const add = (key, text) => items.push({ key, ...BELL[key], ...(text ? { text } : {}) });
    if (st.token_rejected) add('rejected');
    else if (!st.server_online) add('offline');
    const chain = st.audit_chain || {};
    if (['rollback', 'rewritten', 'edited'].includes(chain.state)) add('chain', chain.detail);
    if (BELL[st.alerts] && st.alerts !== 'ok') add(st.alerts);
    if (sealed.open) add('sealed', 'Sealed content is open for about ' + minutes(sealed.left) + ' more minute' + (minutes(sealed.left) === 1 ? '' : 's') + '.');
    const recent = (st.sealed_content && st.sealed_content.alerts) || [];
    recent.filter((a) => !BELL_SKIP_KINDS.includes(a.kind) && !dismissed.includes('alert-' + a.kind + '-' + a.at)).sort((a, b) => b.at - a.at).forEach((a) => {
      items.push({ key: 'alert-' + a.kind + '-' + a.at, severity: 'info', ...(BELL_KIND_FIX[a.kind] || BELL_KIND_DEFAULT), text: a.text, at: a.at });
    });
    const order = ['critical', 'warning', 'info'];
    return items.sort((a, b) => order.indexOf(a.severity) - order.indexOf(b.severity));
  }

  // ------------------------------------------------------------- Alerts page (plan step 4.4, rules 33, 34, 36, 37, 38)

  // The bell's items, grouped Critical, Warning, Info, drawn as rows on one grid (see .alerts-row). A condition has no time
  // (offline, rejected, ...): it says "ongoing" rather than a made-up time. Only Info can be dismissed; the others clear
  // themselves when the condition ends. Redrawn by showBell, which ui.live calls with every changed status.
  const ALERT_GROUPS = [['critical', 'Critical', 'error'], ['warning', 'Warning', 'warning'], ['info', 'Info', 'info']];
  let alertsShown = false;
  let lastStatus = null;
  function alertTime(i) {
    const t = i.at ? ui.time(i.at) : ui.el('span', { text: 'ongoing' });
    t.classList.add('alerts-time');
    return t;
  }
  function renderAlertsPage(items) {
    const body = $('alerts-body');
    if (!body) return;
    const count = (sev) => items.filter((i) => i.severity === sev).length;
    $('alerts-sum').textContent = items.length
      ? count('critical') + ' critical, ' + count('warning') + (count('warning') === 1 ? ' warning, ' : ' warnings, ') + count('info') + ' info'
      : '';
    for (const [sev, word, state] of ALERT_GROUPS) {
      const group = items.filter((i) => i.severity === sev);
      const section = $('alerts-' + sev);
      section.hidden = group.length === 0;
      section.querySelector('.alerts-n').textContent = '· ' + group.length;
      ui.rows(section.querySelector('.alerts-list'), group, (i) => ui.el('div', { class: 'alerts-row alerts-row-' + sev },
        ui.status(state, word),
        ui.el('span', { class: 'alerts-text', text: i.text }),
        alertTime(i),
        ui.el('a', { class: 'alerts-fix', href: i.href, text: i.fix }),
        ui.el('span', { class: 'alerts-act' }, sev === 'info'
          ? ui.el('button', { type: 'button', class: 'btn btn-quiet btn-icon', 'aria-label': 'Dismiss: ' + i.text, 'data-tip': 'Dismiss', 'data-dismiss': i.key,
            onclick: () => dismissInfo(i.key) }, ui.alertIcon('close'))
          : 'Clears itself when fixed')),
      { key: (i) => i.key, fresh: alertsShown, noun: ['alert', 'alerts'] });
    }
    alertsShown = true;
    const empty = $('alerts-empty');
    empty.hidden = items.length > 0;
    if (!empty.firstChild) empty.append(ui.empty('No alerts'));
  }
  // The row goes at once; focus moves to the next Info row's button, or to the summary line.
  function dismissInfo(key) {
    const buttons = [...document.querySelectorAll('#alerts-info [data-dismiss]')];
    const at = buttons.findIndex((b) => b.dataset.dismiss === key);
    dismissAlert(key);
    if (lastStatus) showBell(lastStatus);
    const rest = [...document.querySelectorAll('#alerts-info [data-dismiss]')];
    (rest[Math.min(at, rest.length - 1)] || $('alerts-sum')).focus();
  }

  // ------------------------------------------------------------- Home: Needs attention (plan step 4.2, rules 7, 8, 12)

  // The list is the bell's critical and warning items (urgentOf(bellItems(status)), the same rows the badge counts, pushed
  // here by showBell) followed by the setup gaps below (always warnings; Network is optional, so a missing host name counts only while HTTPS is on). No API is added: the gaps come from GETs the other
  // pages already make. Each rule: `test(data)` is true when the gap exists; `data` holds the answers (a read that failed
  // is null, and a rule that needs it is skipped, so a failed read never invents a gap). `fix` is the link's label.
  const SETUP_GAPS = [
    { key: 'no-provider', test: (d) => d.providers && d.providers.length === 0, text: 'No provider is set up, so nobody can ask anything yet.', fix: 'Add provider', href: '/admin/providers/#add-provider' },
    { key: 'no-model', test: (d) => d.providers && d.providers.length > 0 && d.catalog && d.catalog.models.length === 0, text: 'No model is approved, so clients have nothing to choose from.', fix: 'Approve models', href: '/admin/providers/#catalog-section' },
    { key: 'no-default-model', test: (d) => d.catalog && d.catalog.models.length > 0 && !d.catalog.default, text: 'No default model is chosen.', fix: 'Choose default', href: '/admin/providers/#catalog-section' },
    { key: 'no-user', test: (d) => d.users && d.users.length === 0, text: 'No user exists yet, so there is nobody to pair an app for.', fix: 'Add user', href: '/admin/clients/#users' },
    { key: 'no-client', test: (d) => d.users && d.users.length > 0 && d.clients && d.clients.length === 0, text: 'No client is paired yet.', fix: 'Pair a client', href: '/admin/clients/#d-user' },
    { key: 'no-backup-key', test: (d) => d.schedule && !d.schedule.enabled, text: 'No backup key is set, so nothing is backed up.', fix: 'Open Backup', href: '/admin/backup/' },
    { key: 'backup-destination', test: (d) => d.schedule && d.schedule.enabled && (!d.schedule.destination.writable || !!d.schedule.destination.problem), text: 'The backup destination cannot be written to.', fix: 'Open Backup', href: '/admin/backup/' },
    { key: 'no-backup-yet', test: (d) => d.schedule && d.schedule.enabled && !d.schedule.last_success_at, text: 'No backup has been made yet.', fix: 'Open Backup', href: '/admin/backup/' },
    { key: 'drill-due', test: (d) => d.schedule && d.schedule.enabled && d.schedule.drill_due, text: 'A restore drill is due: restore a backup into a throwaway install to be sure it works.', fix: 'Open Backup', href: '/admin/backup/' },
    { key: 'no-hostname', test: (d) => d.network && d.network.tls_enabled && !d.network.hostname, text: 'HTTPS is on but no host name is set.', fix: 'Open Network', href: '/admin/network/#n-host' },
  ];
  const setupGapItems = (data) => SETUP_GAPS.filter((g) => g.test(data)).map((g) => ({ key: 'gap-' + g.key, severity: 'warning', source: 'setup', text: g.text, fix: g.fix, href: g.href }));

  // Reads of the setup answers; a refusal (an answer with a status) leaves that part null, a lost connection throws so
  // ui.live backs off and shows "Reconnecting…".
  const GAP_READS = {
    providers: '/api/admin/v1/providers', catalog: '/api/admin/v1/catalog', users: '/api/admin/v1/users', clients: '/api/admin/v1/clients',
    schedule: '/api/admin/v1/backup/schedule', network: '/api/admin/v1/network',
  };
  // Home reads the same answers for its summaries, plus the stats of the window and the services (the menu's health data).
  const homeReads = () => ({
    ...GAP_READS,
    stats: '/api/admin/v1/audit/stats?since=' + (Math.floor(Date.now() / MS) - K.HOME_STATS_WINDOW_SECS),
    services: '/api/admin/v1/services',
  });
  async function readSetup() {
    const entries = await Promise.all(Object.entries(homeReads()).map(async ([key, path]) => {
      try { return [key, await api('GET', path)]; } catch (ex) { if (ex.status === undefined) throw ex; return [key, null]; }
    }));
    return Object.fromEntries(entries);
  }

  let bellNow = null; // the bell's items, from the last status
  let gapsNow = null; // the setup gaps, from the last read
  let attentionShown = false;
  function renderAttention() {
    const list = $('attention-list');
    if (!list || !bellNow || !gapsNow) return;
    const rank = (i) => (i.severity === 'critical' ? 0 : 1);
    const items = [...urgentOf(bellNow).map((i) => ({ ...i, source: 'alert' })), ...gapsNow].sort((a, b) => rank(a) - rank(b));
    $('attention-count').textContent = items.length === 1 ? '1 item' : items.length + ' items';
    ui.rows(list, items, (i) => ui.el('div', { class: 'arow arow-' + i.severity, 'data-source': i.source },
      ui.status(i.severity === 'critical' ? 'error' : 'warning', i.severity === 'critical' ? 'Critical' : 'Warning'),
      ui.el('span', { class: 'arow-text', text: i.text }),
      ui.el('a', { class: 'btn btn-secondary btn-s', href: i.href, text: i.fix })), { key: (i) => i.key, fresh: attentionShown, noun: ['item needing attention', 'items needing attention'] });
    attentionShown = true;
    const empty = $('attention-empty');
    empty.hidden = items.length > 0;
    if (!empty.firstChild) empty.append(ui.empty('Nothing needs attention'));
  }

  // ------------------------------------------------------------- Home: numbers, chart and summaries (plan step 4.3)

  // The one reader of GET /audit/stats on Home (audit.js reads the same document for its overview; it is not loaded here).
  // Every figure is a number or null; null is shown as "–", never as 0 (a cost the server could not price is "unknown").
  function statsFigures(s) {
    if (!s) return { requests: null, errorRate: null, tokens: null, cost: null, costUnknown: false, p95: null, days: [] };
    const tokens = s.tokens || {};
    return {
      requests: s.requests == null ? null : s.requests,
      errorRate: s.error_rate == null ? null : Math.round(s.error_rate * 1000) / 10,
      tokens: s.tokens ? (tokens.in || 0) + (tokens.out || 0) : null,
      cost: s.cost == null ? null : s.cost,
      costUnknown: s.cost == null && s.cost_unknown_requests > 0,
      p95: s.latency_ms && s.latency_ms.p95 != null ? s.latency_ms.p95 : null,
      days: (s.by_day || []).map((d) => ({ day: d.day, requests: d.requests })),
    };
  }
  const dash = () => ui.el('span', { class: 'num', text: '–' });
  const dayLabel = (day) => new Date(day + 'T00:00:00Z').toLocaleDateString('en-US', { month: 'short', day: 'numeric', timeZone: 'UTC' });
  function latencyNum(ms) {
    if (ms == null) return dash();
    return ms >= MS ? ui.num(Math.round(ms / 100) / 10, 's') : ui.num(Math.round(ms), 'ms');
  }

  function renderKpis(f) {
    const put = (id, node) => $(id).replaceChildren(node);
    put('kpi-requests', f.requests == null ? dash() : ui.num(f.requests));
    put('kpi-errors', f.errorRate == null ? dash() : ui.num(f.errorRate, '%'));
    put('kpi-tokens', f.tokens == null ? dash() : ui.num(f.tokens, 'tokens'));
    put('kpi-cost', f.cost != null ? ui.num(f.cost) : ui.el('span', { class: 'num', text: f.costUnknown ? 'unknown' : '–' }));
    put('kpi-latency', latencyNum(f.p95));
  }

  // The chart is CSS bars (heights set through the CSSOM) in a role="img" whose label is the visible one-line summary; the
  // same values are in a visually hidden table. A read that failed leaves the numbers as they were.
  function renderChart(days) {
    const summary = $('chart-summary');
    const chart = $('home-chart');
    const wrap = $('chart-table-wrap');
    const span = K.HOME_STATS_WINDOW_DAYS;
    chart.replaceChildren();
    $('chart-table').tBodies[0].replaceChildren();
    if (!days.length) {
      summary.textContent = 'No requests in the last ' + span + ' days.';
      chart.hidden = true; wrap.hidden = true;
      chart.setAttribute('aria-label', '');
      return;
    }
    const fmt = (n) => Number(n).toLocaleString();
    const total = days.reduce((a, d) => a + d.requests, 0);
    const peak = days.reduce((a, d) => (d.requests > a.requests ? d : a), days[0]);
    const low = Math.min(...days.map((d) => d.requests));
    const text = days.length === 1
      ? fmt(total) + ' requests on ' + dayLabel(days[0].day) + '.'
      : fmt(total) + ' requests over ' + days.length + ' days, from ' + fmt(low) + ' to ' + fmt(peak.requests) + ' a day, busiest on ' + dayLabel(peak.day) + '.';
    summary.textContent = text;
    chart.setAttribute('aria-label', text);
    const max = peak.requests || 1;
    const plot = ui.el('div', { class: 'chart-plot' });
    for (const d of days) {
      const bar = ui.el('div', { class: 'chart-bar', 'data-day': d.day, 'data-requests': d.requests }, ui.el('span', { class: 'chart-val', text: fmt(d.requests) }));
      bar.style.height = (d.requests / max * 100) + '%';
      plot.append(ui.el('div', { class: 'chart-col' }, bar));
    }
    const y = ui.el('div', { class: 'chart-y' }, ui.el('span', { text: fmt(max) }), ui.el('span', { text: '0' }));
    y.children[0].style.bottom = '100%';
    y.children[1].style.bottom = '0';
    chart.append(y, plot, ui.el('div', { class: 'chart-x' }, ui.el('span', { text: dayLabel(days[0].day) }), days.length > 1 ? ui.el('span', { text: dayLabel(days[days.length - 1].day) }) : null));
    chart.hidden = false;
    $('chart-table').tBodies[0].append(...days.map((d) => ui.el('tr', {}, ui.el('td', { text: dayLabel(d.day) }), ui.el('td', { text: fmt(d.requests) }))));
    wrap.hidden = false;
  }

  function renderServices(services) {
    const list = $('home-services-list');
    if (!services) { list.replaceChildren(ui.el('p', { class: 'hint', text: 'Services not checked.' })); return; }
    const titleOf = (name) => (SERVICE_INFO[name] ? SERVICE_INFO[name].title : name);
    ui.rows(list, services.services, (svc) => ui.el('div', { class: 'srow' },
      ui.el('span', { text: titleOf(svc.name) }),
      svc.state === 'running' ? ui.status('ok', 'Running') : ui.status('warning', svc.state.charAt(0).toUpperCase() + svc.state.slice(1))),
    { key: (svc) => svc.name });
  }

  const fact = (dl, label, node) => dl.append(ui.el('dt', { text: label }), ui.el('dd', {}, node));
  function renderBackupSummary(v) {
    const dl = $('backup-facts');
    dl.replaceChildren();
    if (!v) { fact(dl, 'Backups', dash()); return; }
    if (!v.enabled) { fact(dl, 'Backups', ui.status('warning', 'No backup key')); return; }
    const failed = v.last_failure_at && (!v.last_success_at || v.last_failure_at > v.last_success_at);
    fact(dl, 'Status', v.missed ? ui.status('warning', 'Overdue') : failed ? ui.status('error', 'Last try failed') : ui.status('ok', 'On schedule'));
    fact(dl, 'Last backup', v.last_success_at ? ui.time(v.last_success_at) : document.createTextNode('none yet'));
    fact(dl, 'Next backup', v.next_run_at ? ui.time(v.next_run_at) : dash());
  }
  function renderClientsSummary(d) {
    const dl = $('clients-facts');
    dl.replaceChildren();
    fact(dl, 'Users', d.users ? ui.num(d.users.length) : dash());
    fact(dl, 'Clients', d.clients ? ui.num(d.clients.length) : dash());
  }

  function renderHome(d) {
    const f = statsFigures(d.stats);
    renderKpis(f);
    if (d.stats) renderChart(f.days); else $('chart-summary').textContent = 'The numbers could not be read.';
    renderServices(d.services);
    renderBackupSummary(d.schedule);
    renderClientsSummary(d);
  }
  let liveAttention = null;
  function loadHome() {
    if (!liveAttention) {
      liveAttention = ui.live(readSetup, {
        when: appShownNow,
        notice: $('attention'),
        render(data) { gapsNow = setupGapItems(data); renderAttention(); renderHome(data); },
        onError(ex) { if (ex.message !== 'Signed out') pageError(ex.message); },
      });
    }
    return liveAttention.start();
  }

  // Quick actions: links to the page that does the work, and Back up now, which is the Backup page's own call.
  on('home-backup', 'click', async (e) => {
    const button = e.currentTarget;
    const done = ui.busy(button, 'Backing up…');
    inlineError('home-error', '');
    try {
      const r = await api('POST', '/api/admin/v1/backup/run');
      ui.toast('Backup made: ' + r.file);
      if (liveAttention) liveAttention.refresh();
    } catch (ex) {
      inlineError('home-error', ex.message);
    } finally { done(); }
  });

  // A critical row pops up once per tab: the keys already shown are kept in sessionStorage (a blocked store only means
  // a pop-up may come back after a reload). A key that is no longer critical is forgotten, so it can pop up again.
  const baseTitle = document.title;
  const POPPED_KEY = 'askbar-admin.alert-pops';
  const popped = new Set();
  try { JSON.parse(sessionStorage.getItem(POPPED_KEY) || '[]').forEach((k) => popped.add(k)); } catch (_) { /* no storage */ }
  function showBell(st) {
    lastStatus = st;
    const items = bellItems(st);
    ui.bell(items);
    bellNow = items;
    renderAttention();
    renderAlertsPage(items);
    // The tab title carries the count the badge shows (critical plus warning), so a hidden tab still says so.
    const urgent = urgentOf(items).length;
    document.title = (urgent ? '(' + urgent + ') ' : '') + baseTitle;
    const critical = items.filter((i) => i.severity === 'critical');
    for (const item of critical) {
      if (popped.has(item.key)) continue;
      popped.add(item.key);
      ui.alertPop(item);
    }
    for (const key of [...popped]) {
      if (critical.some((i) => i.key === key)) continue;
      popped.delete(key);
      ui.alertPopClear(key);
    }
    try { sessionStorage.setItem(POPPED_KEY, JSON.stringify([...popped])); } catch (_) { /* no storage */ }
  }

  // The "server offline" banner: ask the admin how its link to the server is, now and then (ui.live: every
  // STATUS_EVERY_SECS while the tab is visible, every HIDDEN_EVERY_SECS while it is hidden, backing off when the admin
  // itself does not answer). It always draws, so a banner another request raised (a 502) clears when the server is back.
  // A page with no live list shows "Reconnecting…" at the top of the page; a list shows its own next to the list.
  const appShownNow = () => !$('app').hidden;
  const listPage = ['services', 'providers', 'clients'].includes(page);
  const liveStatus = ui.live(() => api('GET', '/api/admin/v1/status', undefined, { etag: true }), {
    every: 'STATUS_EVERY_SECS',
    hidden: 'slow',
    always: true,
    when: appShownNow,
    notice: listPage ? null : page === 'alerts' ? $('alerts-body') : $('page-error'),
    render: showStatus,
  });
  const checkServer = () => liveStatus.refresh();
  function showStatus(st) {
    // A refused token is the more useful message: the server is there, but it does not accept this admin.
    ui.banner('rejected', 'error', st.token_rejected ? REJECTED_PARTS : null);
    ui.banner('offline', 'error', !st.server_online && !st.token_rejected ? OFFLINE_TEXT : null);
    // The audit chain no longer matches what this admin stored (ADR 0010 rule 7). The sentence is the admin's, text only.
    const chain = st.audit_chain || {};
    const bad = ['rollback', 'rewritten', 'edited'].includes(chain.state);
    ui.banner('chain', 'error', bad ? chain.detail : null);
    showAlertsBanner(st.alerts);
    showSealed(st.sealed_content || {});
    showBell(st);
    checkKill();
  }

  // The status bar's kill switch (plan step 3.1, rule 3): read-only, a word and a symbol (rule 37), from the same
  // settings the Settings page edits. The switch itself stays a field of that form.
  async function checkKill() {
    const box = $('sb-kill');
    if (!box) return;
    let state = 'pending';
    let text = 'Kill switch: unknown';
    try {
      const s = await api('GET', '/api/admin/v1/settings');
      state = s.kill_switch ? 'error' : 'ok';
      text = s.kill_switch ? 'Kill switch: on' : 'Kill switch: off';
    } catch (_) { /* signed out, or the admin itself is away */ }
    box.className = 'status status-' + state;
    box.querySelector('.status-word').textContent = text;
  }

  // The side menu's health line (plan step 2.2): how many of the services answer, as a word and a symbol
  // (rule 37), linking to the Services page. Read every LIVE_EVERY_SECS while the tab is visible (ui.live).
  function showHealth(state, text) {
    const box = $('menu-health-status');
    if (!box) return;
    box.className = 'status status-' + state;
    box.querySelector('.status-word').textContent = text;
    $('menu-health').dataset.tip = text;
  }
  const liveHealth = ui.live(() => api('GET', '/api/admin/v1/services', undefined, { etag: true }), {
    when: appShownNow,
    render: ({ services }) => {
      const down = services.filter((s) => s.state !== 'running').length;
      showHealth(down ? 'warning' : 'ok', down ? down + ' of ' + services.length + ' services not running' : 'All ' + services.length + ' services running');
    },
    // Signed out, or the admin itself is away: say it is not checked.
    onError: () => showHealth('pending', 'Services not checked'),
  });
  function showApp() {
    hideAll();
    show($('app'), true);
    appShown = true;
    loadPage();
    focusHash();
    initSearch();
    liveStatus.start();
    liveHealth.start();
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

  // ---------------------------------------------------------- global search

  // The status bar's search (plan step 3.5, rule 4). It finds, in this order: pages, settings (with the value now),
  // people, clients, providers, models, Audit filters, and last Reference (empty until step 7.4 fills `reference`).
  // Pages come from the menu itself; everything else is read with GETs on the first focus of the field (and again when
  // the last read is older than LIVE_EVERY_SECS), never on page load. No API is added.
  const SEARCH_READS = {
    settings: '/api/admin/v1/settings',
    network: '/api/admin/v1/network',
    keys: '/api/admin/v1/keys',
    schedule: '/api/admin/v1/backup/schedule',
    backupKey: '/api/admin/v1/backup/key',
    users: '/api/admin/v1/users',
    clients: '/api/admin/v1/clients',
    providers: '/api/admin/v1/providers',
    catalog: '/api/admin/v1/catalog',
  };
  const withUnit = (v, unit) => (v === null || v === undefined ? 'default' : v + ' ' + unit);
  const onOff = (v) => (v ? 'on' : 'off');
  // One row per setting that exists today: [id of its field on the page, its label, the page's slug, which read above
  // holds it, how to turn that response into the value shown (null: no value to show)]. Enter opens the page at #id.
  // When a page is split in a later phase (step 5.x), change the slug here; nothing else needs to know.
  const SETTING_FIELDS = [
    ['s-prompt', 'System prompt', 'general', 'settings', (r) => (r.system_prompt ? [...r.system_prompt].length + ' characters' : 'empty')],
    ['s-gen-timeout', 'Reply time limit', 'general', 'settings', (r) => withUnit(r.generation_timeout_secs, 's')],
    ['s-history-turns', 'Earlier questions sent with each question', 'general', 'settings', (r) => r.history_turns || 'all'],
    ['s-kill-switch', 'Kill switch', 'general', 'settings', (r) => onOff(r.kill_switch)],
    ['s-kill-aborts', 'Stop running replies when the kill switch goes on', 'general', 'settings', (r) => onOff(r.kill_switch_aborts_running !== false)],
    ['s-retention', 'Keep audit records for', 'general', 'settings', (r) => withUnit(r.audit_retention_days, 'days')],
    ['s-unseal-idle', 'Seal after idle minutes', 'general', 'settings', (r) => withUnit(r.unseal_idle_minutes, 'min')],
    ['s-unseal-total', 'Seal after minutes in all', 'general', 'settings', (r) => withUnit(r.unseal_total_minutes, 'min')],
    ['s-notice-on', 'Privacy notice', 'general', 'settings', (r) => (r.privacy_notice_enabled !== false ? 'shown' : 'hidden')],
    ['s-warning-on', 'AI warning above the input box', 'general', 'settings', (r) => (r.ai_warning_enabled !== false ? 'shown' : 'hidden')],
    ['s-theme', 'Theme', 'general', 'settings', (r) => r.display.theme],
    ['s-width', 'Window width', 'general', 'settings', (r) => r.display.width + ' px'],
    ['s-max-height', 'Window max height', 'general', 'settings', (r) => r.display.max_height + ' px'],
    ['s-history-width', 'History panel width', 'general', 'settings', (r) => r.display.history_width + ' px'],
    ['s-top', 'Top position', 'general', 'settings', (r) => r.display.top_percent],
    ['s-hide-on-blur', 'Hide the window when it loses focus', 'general', 'settings', (r) => onOff(r.display.hide_on_blur)],
    ['s-newest-first', 'Show conversations newest first', 'general', 'settings', (r) => onOff(r.display.newest_first)],
    ['n-tls', 'Serve over HTTPS', 'network', 'network', (r) => onOff(r.tls_enabled)],
    ['n-host', 'Hostname', 'network', 'network', (r) => r.hostname || 'not set'],
    ['n-challenge', 'How to prove the hostname to Let\'s Encrypt', 'network', 'network', (r) => (r.acme_challenge === 'http' ? 'HTTP-01' : 'DNS-01')],
    ['n-allow', 'Only let these addresses open the admin site', 'network', 'network', (r) => onOff(r.allowlist_enabled)],
    ['n-cidr', 'Allowed addresses and networks', 'network', 'network', (r) => r.admin_cidr || 'none'],
    ['n-staging', 'Use the Let\'s Encrypt staging CA', 'network', 'network', (r) => onOff(r.acme_staging)],
    ['pw-current', 'Change admin password', 'security', null, null],
    ['key-name', 'Admin API key', 'security', 'keys', (r) => (r.length ? r[0].name : 'none')],
    ['sched-keep-files', 'Keep at most this many backups', 'backup', 'schedule', (r) => r.keep_files],
    ['sched-keep-days', 'Delete backups older than this many days', 'backup', 'schedule', (r) => r.keep_days],
    ['bk-pass', 'Backup key', 'backup', 'backupKey', (r) => (r.set ? 'fingerprint ' + r.fingerprint : 'not set')],
  ];
  // Audit has no filter in its address yet (audit.js reads only `view` and `id`), so these open the requests view and
  // the row says so; the filter is then chosen there. Statuses are the Status filter's own words.
  const AUDIT_STATUSES = ['done', 'error', 'cancelled', 'timed out', 'interrupted'];
  const AUDIT_NOTE = 'Audit / Model calls · opens the requests view, pick the filter there';
  const AUDIT_REQUESTS = '/admin/audit/calls/?view=requests';

  const search = { data: {}, at: 0, loading: null, combo: null, pages: [] };

  function searchPages() {
    return [...document.querySelectorAll('#nav a[data-tab]')].map((a) => {
      const sub = a.closest('.sm-subs');
      const group = sub && sub.previousElementSibling ? sub.previousElementSibling.textContent.trim() : '';
      const title = a.textContent.trim();
      return { title, group, where: group && group !== title ? group : 'Page', href: a.getAttribute('href'), slug: a.dataset.tab, nav: a.querySelector('svg') };
    });
  }

  // The rows of each group for the data read so far. A read that failed leaves its rows out.
  const SEARCH_GROUPS = [
    ['pages', 'Pages', () => search.pages.map((p) => ({ ...p, icon: 'nav' }))],
    ['settings', 'Settings', () => {
      const where = (slug) => { const p = search.pages.find((x) => x.slug === slug); return p ? (p.group && p.group !== p.title ? p.group + ' / ' + p.title : p.title) : slug; };
      return SETTING_FIELDS.map(([id, title, slug, source, read]) => {
        let value = '';
        const r = source && search.data[source];
        if (r && read) { try { value = String(read(r)); } catch (_) { value = ''; } }
        return { title, where: where(slug), value, href: '/admin/' + slug + '/#' + id, icon: 'settings' };
      });
    }],
    ['people', 'People', () => (search.data.users || []).map((u) => ({ title: u.name, where: 'Settings / Clients · users', value: u.paused ? 'paused' : '', href: '/admin/clients/#users', icon: 'people' }))],
    ['clients', 'Clients', () => (search.data.clients || []).map((c) => {
      const user = (search.data.users || []).find((u) => u.id === c.user_id);
      return { title: c.name, where: 'Settings / Clients' + (user ? ' · ' + user.name : ''), value: c.revoked ? 'revoked' : 'active', href: '/admin/clients/#clients', icon: 'people' };
    })],
    ['providers', 'Providers', () => (search.data.providers || []).map((p) => ({ title: p.name, where: 'Settings / Providers · ' + p.kind, value: '', href: '/admin/providers/#providers', icon: 'providers' }))],
    ['models', 'Models', () => {
      const c = search.data.catalog || { models: [] };
      const name = (id) => { const p = (search.data.providers || []).find((x) => x.id === id); return p ? p.name : 'a provider that is gone'; };
      return c.models.map((m) => ({ title: m.model, where: 'Settings / Providers · ' + name(m.provider_id), value: c.default === m.provider_id + '/' + m.model ? 'default' : '', href: '/admin/providers/#catalog-approved', icon: 'providers' }));
    }],
    ['audit', 'Audit', () => [
      ...AUDIT_STATUSES.map((w) => ({ title: 'Status: ' + w, where: AUDIT_NOTE, value: '', href: AUDIT_REQUESTS, icon: 'audit' })),
      ...(search.data.providers || []).map((p) => ({ title: 'Provider: ' + p.name, where: AUDIT_NOTE, value: '', href: AUDIT_REQUESTS, icon: 'audit' })),
    ]],
    // Step 7.4 fills this from the reference articles; until then the group has no rows and is not drawn.
    ['reference', 'Reference', () => []],
  ];

  function searchGroups(query) {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    return SEARCH_GROUPS.map(([key, label, items]) => ({
      key,
      label,
      items: items().filter((i) => { const hay = (i.title + ' ' + i.where).toLowerCase(); return words.every((w) => hay.includes(w)); }).slice(0, K.SEARCH_RESULTS_PER_GROUP),
    }));
  }

  function searchIcon(item) {
    if (item.icon === 'nav') return item.nav ? item.nav.cloneNode(true) : null;
    const holder = $('search-icons');
    const src = holder && holder.content.querySelector('[data-icon="' + item.icon + '"]');
    return src && src.firstElementChild ? src.firstElementChild.cloneNode(true) : null;
  }

  // Read the sources the first time the field is focused, and again when the last read is older than LIVE_EVERY_SECS.
  function loadSearchData() {
    if (search.loading || (search.at && Date.now() - search.at < K.LIVE_EVERY_SECS * MS)) return;
    search.loading = Promise.allSettled(Object.entries(SEARCH_READS).map(async ([key, path]) => { search.data[key] = await api('GET', path); }))
      .then(() => { search.at = Date.now(); search.loading = null; if (search.combo && document.activeElement === $('gs-input')) search.combo.render(); });
  }

  // On a phone the field lives at the top of the full-screen menu; elsewhere it follows the breadcrumb. "Is the Menu
  // button shown" is the phone check (the same one the menu uses), so the breakpoint stays in the CSS.
  const onPhone = () => { const b = $('menu-open'); return !!b && b.getClientRects().length > 0; };
  function placeSearch() {
    const gs = $('gs');
    if (!gs) return;
    if (onPhone()) { if (gs.parentElement.id !== 'side-menu') $('menu-health').before(gs); }
    else if (!gs.parentElement.classList.contains('gs-left')) document.querySelector('.gs-left').append(gs);
  }

  // Scroll to and focus what the address names (#s-retention): a search result, or a link from the bell.
  function focusHash() {
    const id = decodeURIComponent(location.hash.slice(1));
    const target = id && $(id);
    if (!target || target.closest('[hidden]')) return;
    target.scrollIntoView({ block: 'center' });
    if (target.matches('input, select, textarea, button, a[href]')) target.focus({ preventScroll: true });
  }
  window.addEventListener('hashchange', focusHash);

  function openResult(item) {
    const url = new URL(item.href, location.href);
    $('gs-input').value = '';
    search.combo.render();
    if (url.pathname === location.pathname && url.search === location.search) {
      if (url.hash === location.hash) focusHash(); else location.hash = url.hash;
      if (onPhone() && document.documentElement.classList.contains('menu-open')) $('menu-open').click();
    } else {
      location.assign(url);
    }
  }

  function initSearch() {
    const input = $('gs-input');
    if (!input || search.combo) return;
    search.pages = searchPages();
    search.combo = ui.combobox({ input, list: $('gs-list'), pop: $('gs-pop'), none: $('gs-none'), groups: searchGroups, icon: searchIcon, choose: openResult });
    input.addEventListener('focus', loadSearchData);
    placeSearch();
    window.addEventListener('resize', placeSearch);
    // "/" focuses the search unless a field has the keys (or a box is open). On a phone it opens the menu first.
    document.addEventListener('keydown', (e) => {
      if (e.key !== '/' || e.ctrlKey || e.metaKey || e.altKey || e.defaultPrevented || $('app').hidden) return;
      const t = e.target;
      if (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) || document.querySelector('dialog[open]')) return;
      e.preventDefault();
      if (onPhone() && !document.documentElement.classList.contains('menu-open')) $('menu-open').click();
      input.focus();
      input.select();
    });
  }

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
        pageError('The admin has not come back within a minute. Check it from the host.');
      }
    } catch (ex) {
      pageError(ex.message);
    }
    servicesBusy = false;
    liveServices.refresh({ force: true });
  }

  // The service cards, kept current every STATUS_EVERY_SECS while the tab is visible, and paused while a restart runs.
  // After the admin's own restart the cards are rebuilt (force), so "Working..." never lingers.
  const liveServices = ui.live(() => api('GET', '/api/admin/v1/services', undefined, { etag: true }), {
    every: 'STATUS_EVERY_SECS',
    when: appShownNow,
    skip: () => servicesBusy,
    notice: $('services'),
    render(data, ctx) {
      ui.rows($('services'), data.services, serviceCard, { key: (svc) => svc.name, force: ctx.force, sig: (svc) => JSON.stringify([svc, servicesBusy]) });
      loadCpa();
      const rec = $('services-recent');
      rec.textContent = data.recent.length ? '' : 'None yet.';
      for (const r of data.recent) {
        const line = document.createElement('div');
        line.textContent = new Date(r.at * 1000).toLocaleString() + ' · ' + r.action + ' ' + r.service + (r.ok ? '' : ' · failed');
        rec.appendChild(line);
      }
    },
    onError(ex) {
      if (ex.message === 'Signed out') return;
      const box = $('services');
      const p = document.createElement('p');
      p.className = 'error';
      p.textContent = ex.message;
      box.replaceChildren(p);
    },
  });
  function loadServices() { return liveServices.start(); }
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
        try { await api('DELETE', '/api/admin/v1/cpa/accounts/' + encodeURIComponent(a.name)); } catch (ex) { pageError(ex.message); }
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
      try { await api('DELETE', '/api/admin/v1/providers/' + encodeURIComponent(p.id)); await loadProviders(); } catch (ex) { pageError(ex.message); }
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

  // The provider cards, current every LIVE_EVERY_SECS while the tab is visible. The approved-models draft below is not
  // touched by a check: it is read when the page opens and after the admin's own change (loadProviders).
  const liveProviders = ui.live(() => api('GET', '/api/admin/v1/providers', undefined, { etag: true }), {
    when: appShownNow,
    notice: $('providers'),
    render(list, ctx) {
      providersById = new Map(list.map((p) => [p.id, p]));
      ui.rows($('providers'), list, providerCard, { fresh: !ctx.first, force: ctx.force, noun: ['provider', 'providers'], empty: 'No providers yet. Add one so clients have something to talk to.' });
    },
    onError(ex) { if (ex.message !== 'Signed out') pageError(ex.message); },
  });
  async function loadProviders() {
    await liveProviders.start();
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
      pageError('');
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
    ui.formErrors(e.target, []);
    const problems = [];
    if ([...$('pw-new').value].length < K.MIN_PASSWORD_CHARS) problems.push({ id: 'pw-new', text: 'The new password must be at least ' + K.MIN_PASSWORD_CHARS + ' characters.' });
    if ($('pw-new').value !== $('pw-new2').value) problems.push({ id: 'pw-new2', text: 'The two new passwords do not match.' });
    if (problems.length) return ui.formErrors(e.target, problems);
    try {
      await api('PUT', '/api/admin/v1/password', { current: $('pw-current').value, new: $('pw-new').value });
      for (const id of ['pw-current', 'pw-new', 'pw-new2']) $(id).value = '';
      ui.toast('Password changed. Other browsers have been signed out.');
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
      revokeArmed = setTimeout(() => { revokeArmed = null; b.textContent = 'Revoke'; }, K.TOAST_SECS * MS);
      return;
    }
    clearTimeout(revokeArmed);
    revokeArmed = null;
    b.textContent = 'Revoke';
    try {
      await api('DELETE', '/api/admin/v1/keys/' + encodeURIComponent(keyId));
      ui.toast('The API key was revoked.');
      await loadKeys();
    } catch (ex) {
      $('key-error').textContent = ex.message;
      show($('key-error'), true);
    }
  });

  // ------------------------------------------------------- backup key (ADR 0013)

  // The Backup page: the key, and the scheduled backups (ADR 0013, D27 to D29). Text goes in by textContent.
  async function loadBackup() {
    await loadBackupKey();
    await loadSchedule();
  }

  async function loadSchedule() {
    const v = await api('GET', '/api/admin/v1/backup/schedule');
    const d = v.destination;
    $('sched-path').textContent = d.path;
    $('sched-state').textContent = d.writable ? 'writable' : 'not writable';
    const warn = $('sched-warn');
    const problems = [];
    if (d.problem) problems.push(d.problem);
    if (d.same_disk) problems.push('This is on the same disk as Askbar, so these backups do not survive that disk. Mount a share or another disk over the backup volume (the guide says how).');
    warn.textContent = problems.join(' ');
    show(warn, problems.length > 0);
    $('sched-last').textContent = !v.enabled
      ? 'No backup key is set, so nothing is backed up.'
      : 'Last backup: ' + (v.last_success_at ? when(v.last_success_at) + ' (' + v.last_success_name + ')' : 'none yet')
        + (v.last_failure_at && (!v.last_success_at || v.last_failure_at > v.last_success_at) ? '. The last try failed (' + when(v.last_failure_at) + '): ' + (v.last_failure_reason || 'no reason given') : '')
        + (v.missed ? '. No backup has succeeded for over 26 hours.' : '');
    $('sched-next').textContent = v.enabled && v.next_run_at ? 'Next: ' + when(v.next_run_at) + ' (03:30 UTC daily).' : '';
    const list = $('sched-files');
    list.replaceChildren();
    for (const f of d.files) {
      const li = document.createElement('li');
      li.textContent = f.name + ' · ' + (f.bytes / 1048576).toFixed(1) + ' MB · ' + when(f.made_at);
      list.appendChild(li);
    }
    $('sched-keep-files').value = v.keep_files;
    $('sched-keep-days').value = v.keep_days;
    $('sched-run').disabled = !v.enabled;
    show($('sched-reminder'), v.drill_due);
    $('sched-reminder-text').textContent = 'Restore drill due: restore a backup into a throwaway install to be sure it works (every ' + v.drill_every_days + ' days). The guide says how.';
    show($('sched-error'), false);
  }

  on('sched-run', 'click', async () => {
    const err = $('sched-error');
    show(err, false);
    try {
      const r = await api('POST', '/api/admin/v1/backup/run');
      ui.toast('Backup made: ' + r.file);
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
    loadSchedule().catch((e) => inlineError('sched-error', e.message));
  });

  on('sched-drill', 'click', async () => {
    try { await api('POST', '/api/admin/v1/backup/drill'); await loadSchedule(); } catch (ex) { inlineError('sched-error', ex.message); }
  });

  on('schedule-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('sched-limits-error');
    show(err, false);
    try {
      await api('PUT', '/api/admin/v1/backup/schedule', { keep_files: Number($('sched-keep-files').value), keep_days: Number($('sched-keep-days').value) });
      ui.toast('Saved.');
      await loadSchedule();
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  // The fingerprint and the public name only: the passphrase is typed, sent and emptied, and nothing here ever holds it.
  async function loadBackupKey() {
    const k = await api('GET', '/api/admin/v1/backup/key');
    $('bk-fingerprint').textContent = k.set ? k.fingerprint : 'none yet';
    $('bk-count').textContent = k.set && k.keys > 1 ? '(' + (k.keys - 1) + ' earlier key' + (k.keys - 1 === 1 ? '' : 's') + ' retired)' : '';
    show($('bk-error'), false);
  }

  on('backup-key-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('bk-error');
    show(err, false);
    const fail = (m) => { err.textContent = m; show(err, true); };
    const pass = $('bk-pass').value;
    if (pass !== $('bk-pass2').value) return fail('The two passphrases do not match.');
    if ([...pass].length < K.MIN_PASSPHRASE_CHARS) return fail('The passphrase must be at least ' + K.MIN_PASSPHRASE_CHARS + ' characters.');
    if (!$('bk-stored').checked) return fail('Confirm that you stored the passphrase: it cannot be recovered.');
    if (!confirm('Rotate the backup key? Backups made from now on need the new passphrase; older ones keep the old one.')) return;
    try {
      await api('POST', '/api/admin/v1/backup/rotate-key', { passphrase: pass, confirm: $('bk-pass2').value });
      for (const id of ['bk-pass', 'bk-pass2']) $(id).value = '';
      $('bk-stored').checked = false;
      await loadBackupKey();
      ui.toast('Backup key rotated. Take a new backup now.');
    } catch (ex) {
      fail(ex.message);
    }
  });

  // ----------------------------------------------------------- restore

  // A restore (ADR 0013): the file is sent as it is (a stream the admin keeps as ciphertext), checked, and only then is the passphrase
  // asked for. The passphrase is typed, sent and emptied; nothing here keeps it. Text goes in by textContent.
  let restoreId = null;

  async function loadRestore() {
    for (const id of ['restore-error', 'restore-pass-error']) show($(id), false);
  }

  async function uploadBundle(file) {
    const res = await fetch('/api/admin/v1/restore/inspect', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'X-Askbar-Admin': '1', 'Content-Type': 'application/x-tar' },
      body: file,
    });
    if (res.status === 401) { showLogin(); throw new Error('Signed out'); }
    const text = await res.text();
    let data = null;
    try { data = JSON.parse(text); } catch (_) { data = null; }
    if (!res.ok) {
      const err = new Error((data && (data.detail || data.error)) || ('Request failed (' + res.status + ')'));
      err.status = res.status;
      err.code = (data && data.error) || null;
      throw err;
    }
    return data;
  }

  on('restore-upload-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('restore-error');
    show(err, false);
    const file = $('restore-file').files[0];
    if (!file) return;
    try {
      const v = await uploadBundle(file);
      restoreId = v.restore_id;
      $('restore-release').textContent = v.release;
      $('restore-date').textContent = new Date(v.created_at * 1000).toLocaleString();
      $('restore-fingerprint').textContent = v.key_fingerprint;
      $('restore-parts').textContent = v.parts.map((p) => p.replace(/\.age$/, '')).join(', ');
      show($('restore-form'), true);
      $('restore-pass').focus();
    } catch (ex) {
      restoreId = null;
      show($('restore-form'), false);
      err.textContent = ex.message;
      show(err, true);
    }
  });

  on('restore-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('restore-pass-error');
    show(err, false);
    if (!restoreId) return;
    if (!confirm('Restore this backup? It replaces what this install holds, and the admin restarts.')) return;
    const pass = $('restore-pass').value;
    try {
      const r = await api('POST', '/api/admin/v1/restore', { restore_id: restoreId, passphrase: pass });
      $('restore-pass').value = '';
      restoreId = null;
      show($('restore-upload-form'), false);
      show($('restore-form'), false);
      $('restore-summary').textContent = 'Restored: ' + r.restored.join(', ') + '.'
        + (r.not_restored.length ? ' Not restored yet: ' + r.not_restored.join(', ') + '.' : '');
      $('restore-note').textContent = r.restarting
        ? 'The admin is restarting. Sign in again with the password the backup\'s install had.'
        : 'Restart the admin (docker compose restart admin) to finish: its own data is swapped in at the restart.';
      show($('restore-done'), true);
    } catch (ex) {
      $('restore-pass').value = '';
      err.textContent = ex.message;
      show(err, true);
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
    $('n-challenge').value = n.acme_challenge || 'dns';
    showChallenge();
    $('n-token').value = '';
    $('n-token').placeholder = n.has_token ? 'unchanged (write-only)' : 'write-only';
    $('n-clear').checked = false;
    show($('n-clear-row'), n.has_token);
    $('n-info').textContent = (n.tls_enabled && n.hostname ? 'The admin site is at https://' + n.hostname + '/admin. ' : '')
      + 'New pairing keys point at ' + n.public_url + '.';
    showProxy(n.proxy);
    show($('network-error'), false);
  }

  // With HTTP-01 there is no token to enter, and the warning about port 80 shows.
  function showChallenge() {
    const http = $('n-challenge').value === 'http';
    show($('n-http-note'), http);
    show($('n-dns-box'), !http);
  }
  on('n-challenge', 'change', showChallenge);

  function showProxy(p) {
    const el = $('n-proxy');
    el.className = p.ok ? 'ok' : (p.at ? 'error' : 'hint');
    el.textContent = 'HTTPS front end: ' + p.message + (p.at ? ' (' + new Date(p.at * 1000).toLocaleTimeString() + ')' : '');
  }

  on('n-apply', 'click', async () => {
    try { showProxy(await api('POST', '/api/admin/v1/network/apply')); } catch (ex) { inlineError('network-error', ex.message); }
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
        acme_challenge: $('n-challenge').value,
        cloudflare_token: $('n-token').value,
        clear_token: $('n-clear').checked,
      });
      await loadNetwork();
      ui.toast('Network settings saved.');
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  // ---------------------------------------------------------- clients

  function when(ts) {
    return ts ? new Date(ts * 1000).toLocaleString() : 'never';
  }

  // The users, their clients and the pairing box's "Connected": one live read of both lists, every LIVE_EVERY_SECS while the
  // tab is visible and right after the admin's own change (loadUsers, loadClients).
  let userNames = {};
  let knownClients = [];
  let pairing = null; // { user, known: Set of client ids before the key was made, done }
  const liveClients = ui.live(() => Promise.all([
    api('GET', '/api/admin/v1/users', undefined, { etag: true }),
    api('GET', '/api/admin/v1/clients', undefined, { etag: true }),
  ]), {
    when: appShownNow,
    notice: $('users'),
    render([users, clients], ctx) {
      renderUsers(users, ctx);
      renderClients(clients, ctx);
    },
    onError(ex) { if (ex.message !== 'Signed out') pageError(ex.message); },
  });
  const loadUsers = () => liveClients.refresh();
  function loadClients() { return liveClients.start(); }

  function renderUsers(users, ctx) {
    userNames = Object.fromEntries(users.map((u) => [u.id, u.name]));
    const box = $('users');
    // One row for each user, with the emergency stop for that person: Pause stops their replies (new and running) until
    // Resume. Everything is set as text. A row with its delete confirmation open is left alone.
    ui.rows(box, users, userRow, {
      fresh: !ctx.first, force: ctx.force, noun: ['user', 'users'], keep: (row) => !!row.querySelector('.confirm-delete'),
      empty: 'No users yet. Add one to pair an app for them.',
    });
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

  function userRow(u) {
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
    return row;
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

  function renderClients(list, ctx) {
    knownClients = list;
    ui.rows($('clients'), list, clientCard, {
      fresh: !ctx.first, force: ctx.force, noun: ['client', 'clients'], empty: 'No clients are paired yet.',
      sig: (d) => JSON.stringify([d, userNames[d.user_id]]),
    });
    // The pairing box: the key is used once, so a client that was not there when it was made, for that user, is the app.
    if (pairing && !pairing.done) {
      const hit = list.find((d) => !pairing.known.has(d.id) && d.user_id === pairing.user);
      if (hit) { pairing.done = true; showPairing('ok', 'Connected', ' ' + hit.name + ' is paired.'); }
    }
  }

  function clientCard(d) {
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
        try { await api('POST', '/api/admin/v1/clients/' + encodeURIComponent(d.id) + '/revoke'); await loadClients(); } catch (ex) { pageError(ex.message); }
      });
      revoke.dataset.action = 'revoke';
      head.appendChild(revoke);
    }
    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = (userNames[d.user_id] || 'unknown user') + ' · paired ' + when(d.created_at) + ' · last seen ' + when(d.last_seen);
    card.append(head, meta);
    return card;
  }

  // The state line of the pairing box: a word and a symbol (rule 37), read out politely.
  function showPairing(state, word, more) {
    $('pairing-status').replaceChildren(ui.status(state, word), more || '');
  }

  on('pair-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('pair-error');
    show(err, false);
    try {
      const user = $('d-user').value;
      const known = new Set(knownClients.map((d) => d.id));
      const r = await api('POST', '/api/admin/v1/users/' + encodeURIComponent(user) + '/pairing-keys', { name: $('d-name').value.trim() });
      $('pairing-string').value = r.pairing;
      pairing = { user, known, done: false };
      showPairing('pending', 'Waiting for the app to pair');
      show($('pairing-out'), true);
    } catch (ex) {
      err.textContent = ex.message;
      show(err, true);
    }
  });

  on('pairing-copy', 'click', async () => {
    const t = $('pairing-string');
    t.select();
    try { await navigator.clipboard.writeText(t.value); ui.toast('Copied.'); } catch (_) { document.execCommand('copy'); ui.toast('Copied.'); }
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
    const m = a.email;
    $('m-held-host').textContent = yesNo(m.host);
    $('m-held-login').textContent = m.user || m.password ? 'yes' : 'no';
    $('m-held-addr').textContent = yesNo(m.from && m.to);
    if (m.host) { $('m-port').value = m.port; $('m-tls').value = m.tls; }
    $('m-login-clear').checked = false;
    for (const id of ['a-error', 'm-error', 'a-test-error']) show($(id), false);
    showAlertsBanner(a.banner);
  }
  function alertsError(ex, id = 'a-error') {
    $(id).textContent = ex.message;
    show($(id), true);
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
      ui.toast('Alert settings saved. Send a test alert to check that it arrives.');
    } catch (ex) { alertsError(ex); }
  });
  // Email. The channel not in the body stays as it is, so saving one never touches the other. A user name or password left empty
  // keeps the stored one; the box removes both.
  on('email-form', 'submit', async (e) => {
    e.preventDefault();
    show($('m-error'), false);
    $('a-result').textContent = '';
    const email = { host: $('m-host').value.trim(), port: parseInt($('m-port').value, 10), tls: $('m-tls').value, from: $('m-from').value.trim(), to: $('m-to').value.trim() };
    if ($('m-login-clear').checked) { email.user = ''; email.password = ''; }
    else {
      if ($('m-user').value.trim()) email.user = $('m-user').value.trim();
      if ($('m-password').value.trim()) email.password = $('m-password').value.trim();
    }
    try {
      await api('PUT', '/api/admin/v1/alerts', { email });
      for (const id of ['m-host', 'm-user', 'm-password', 'm-from', 'm-to']) $(id).value = '';
      await loadAlerts();
      ui.toast('Email settings saved. Send a test alert to check that it arrives.');
    } catch (ex) { alertsError(ex, 'm-error'); }
  });
  on('m-remove', 'click', async () => {
    if (!confirm('Stop delivering alerts by email?')) return;
    show($('m-error'), false);
    try {
      await api('PUT', '/api/admin/v1/alerts', { email: null });
      await loadAlerts();
      ui.toast('Email delivery removed.');
    } catch (ex) { alertsError(ex, 'm-error'); }
  });
  on('a-test', 'click', async () => {
    show($('a-test-error'), false);
    $('a-result').textContent = 'Sending a test alert...';
    try {
      const r = await api('POST', '/api/admin/v1/alerts/test');
      const why = { unreachable: 'the server did not answer', refused: 'the server refused it', unauthorized: 'the token or login was not accepted', timeout: 'the server took too long', tls: 'the secure connection could not be made' };
      $('a-result').textContent = r.results.length
        ? r.results.map((c) => c.channel + ': ' + (c.ok ? 'delivered, check ' + (c.channel === 'email' ? 'your inbox' : 'your phone') : 'failed, ' + (why[c.problem] || 'unknown problem'))).join('. ')
        : 'No delivery channel is set up, so nothing was sent.';
    } catch (ex) { $('a-result').textContent = ''; alertsError(ex, 'a-test-error'); }
  });
  on('a-remove', 'click', async () => {
    if (!confirm('Stop delivering alerts through ntfy? Unless email is set up, nothing will tell you when something needs attention until you set it up again.')) return;
    show($('a-error'), false);
    try {
      await api('PUT', '/api/admin/v1/alerts', { ntfy: null });
      $('a-result').textContent = '';
      await loadAlerts();
      ui.toast('ntfy delivery removed.');
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
      ui.toast('Settings saved. Clients pick them up on their next message or when their window opens.');
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
    if ([...next].length < K.MIN_PASSPHRASE_CHARS) return fail('The new passphrase must be at least ' + K.MIN_PASSPHRASE_CHARS + ' characters.');
    try {
      const r = await api('POST', '/api/admin/v1/audit/rotate-key', { current: cur, new: next, confirm: $('r-new2').value });
      for (const id of ['r-current', 'r-new', 'r-new2']) $(id).value = '';
      $('s-key-id').textContent = r.key_id;
      ui.toast('The audit key was rotated. New key id ' + r.key_id + '. Content sealed before still needs the old passphrase.');
    } catch (ex) {
      $('r-current').value = '';
      fail(ex.message);
    }
  });

  // ------------------------------------------------------------- boot

  (async () => {
    try {
      K = await ui.ready;
    } catch (_) {
      pageError('The admin could not load its settings. Reload the page.');
      return;
    }
    route();
  })();
})();
