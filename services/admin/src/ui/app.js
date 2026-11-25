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

  // A read is never counted as the person's activity: the session has a fixed expiry that no request moves (auth.rs),
  // and only changes are recorded (actions.rs). `opts.raw` sends `body` as is, with that content type (an upload).
  async function api(method, path, body, opts = {}) {
    const res = await fetch(path, {
      method,
      credentials: 'same-origin',
      headers: { 'X-Askbar-Admin': '1', ...(body !== undefined ? { 'Content-Type': opts.raw || 'application/json' } : {}) },
      body: body === undefined ? undefined : opts.raw ? body : JSON.stringify(body),
    });
    if (res.status === 401 && !(method === 'POST' && path === '/api/admin/v1/session')) { showLogin(); throw new Error('Signed out'); }
    let data = null;
    const text = await res.text();
    if (text) { try { data = JSON.parse(text); } catch (_) { data = null; } }
    if (res.status === 502) ui.banner('offline', 'error', OFFLINE_TEXT);
    if (!res.ok) {
      // A problem+json body: `detail` is the sentence for a person, `error` the stable code.
      const err = new Error((data && (data.detail || data.error)) || ('Request failed (' + res.status + ')'));
      err.status = res.status;
      err.code = (data && data.error) || null;
      throw err;
    }
    return data;
  }

  // Success goes to a toast; an error stays on the page (rule 32): inline next to the action (ui.inlineError), or the one
  // page-level line under the status bar (pageError). '' clears it.
  const OFFLINE_TEXT = "Server offline. The admin can't reach the Askbar server, so changes, tests, clients and backups won't work until it is back. Nothing you change now is lost or half-saved.";
  function pageError(text) {
    const line = $('page-error');
    line.textContent = text;
    show(line, !!text);
  }
  const pageFailed = (ex) => { if (ex.message !== 'Signed out') pageError(ex.message); };

  // ------------------------------------------------------------- pages

  // One admin page per URL (plan step 1.16): the nav is plain links, and this script runs only the
  // current page's loader. A page with no entry here needs none; audit_calls.js and audit.js register the Audit pages'.
  const page = document.body.dataset.page;
  const tabs = [
    { id: 'home', load: loadHome },
    { id: 'providers', load: loadProviders },
    { id: 'mcp', load: loadMcp },
    { id: 'services', load: loadServices },
    { id: 'people', load: loadClients },
    { id: 'general', load: loadSettings },
    { id: 'network', load: loadNetwork },
    { id: 'security', load: loadSecurity },
    { id: 'backup', load: loadBackup },
    { id: 'notifications', load: loadAlerts },
  ];
  let appShown = false;
  function registerTab(tab) {
    tabs.push(tab);
    if (appShown && tab.id === page) loadPage();
  }
  window.askbarAdmin = { api, registerTab, show, $, openSession, closeSession };

  function loadPage() {
    pageError('');
    const t = tabs.find((x) => x.id === page);
    if (t && t.load) t.load().catch((e) => pageError(e.message));
  }

  // ------------------------------------------------------------ login

  function hideAll() {
    for (const id of ['app', 'login', 'setup']) show($(id), false);
  }

  // The progress list above the setup screens (plan step 6.4): the steps come from its markup, so the count is not typed here.
  // `step` is the position of the screen's step; a step before it is done, that one is current (or done on its "ready" screen).
  // The state is a symbol plus a word, and the current step carries aria-current="step".
  function setStep(step, finished) {
    [...$('setup-steps').children].forEach((li, i) => {
      const state = i < step - 1 || (finished && i === step - 1) ? 'done' : i === step - 1 ? 'current' : 'next';
      li.className = 'setup-step setup-step-' + state;
      if (state === 'current') li.setAttribute('aria-current', 'step'); else li.removeAttribute('aria-current');
      li.querySelector('.setup-step-sym').textContent = state === 'done' ? '\u2713' : state === 'current' ? '\u25cf' : '\u25cb';
      li.querySelector('.setup-step-word').textContent = state === 'done' ? 'Done' : state === 'current' ? 'Current step' : 'Next';
    });
  }

  // Empties password fields and their live counts once the values are sent.
  function clearSecrets(ids) {
    for (const id of ids) {
      const f = $(id);
      f.value = '';
      f.dispatchEvent(new Event('input'));
    }
  }

  // The checks the form can make before it sends anything (rule 26): each is a field error, the first one focused.
  function setupProblems(form, problems) {
    ui.formErrors(form, problems, { title: problems.length === 1 ? 'Fix this to go on:' : 'Fix these ' + problems.length + ' things to go on:' });
    return problems.length > 0;
  }

  // First run (ADR 0007): signed in with the published password, change it before anything else.
  function showPasswordChange() {
    hideAll();
    show($('setup'), true);
    for (const id of ['setup-audit-form', 'setup-audit-done', 'setup-backup-form', 'setup-backup-done']) show($(id), false);
    show($('setup-password-form'), true);
    setStep(1);
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
    // The Restore section of Backup (opened from the first-run screen by its address) is the way out of the setup screens: a restore
    // is for an install nothing has been set up in.
    if (page === 'backup' && location.hash === '#restore-section' && (status.audit_key_set === false || status.backup_key_set === false)) return showApp();
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
    setStep(2);
    $('setup-audit').focus();
  }

  function showBackupSetup() {
    hideAll();
    show($('setup'), true);
    for (const id of ['setup-password-form', 'setup-audit-form', 'setup-audit-done', 'setup-backup-done']) show($(id), false);
    show($('setup-backup-signout'), false);
    show($('setup-backup-form'), true);
    setStep(3);
    $('setup-backup').focus();
  }

  // The backup passphrase (ADR 0013, D22): the same rules as the audit one, and it needs a sign-in within the last 10 minutes, so a
  // slow first run is told to sign in again. The passphrase fields are emptied the moment it is sent.
  on('setup-backup-form', 'submit', async (e) => {
    e.preventDefault();
    const form = e.target;
    const err = $('setup-backup-form-error');
    show(err, false);
    const problems = ui.pairProblems('setup-backup', K.MIN_PASSPHRASE_CHARS, 'passphrase');
    if (!$('setup-backup-stored').checked) problems.push({ id: 'setup-backup-stored', text: 'Confirm that you stored the passphrase: it cannot be recovered.' });
    if (setupProblems(form, problems)) return;
    const fail = (m) => { err.textContent = m; show(err, true); };
    try {
      const r = await api('PUT', '/api/admin/v1/setup/backup-key', { passphrase: $('setup-backup').value, confirm: $('setup-backup2').value });
      clearSecrets(['setup-backup', 'setup-backup2']);
      $('setup-backup-stored').checked = false;
      $('setup-backup-fingerprint').textContent = r.fingerprint;
      show($('setup-backup-form'), false);
      show($('setup-backup-done'), true);
      setStep(3, true);
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
    const form = e.target;
    const err = $('setup-audit-form-error');
    show(err, false);
    const problems = ui.pairProblems('setup-audit', K.MIN_PASSPHRASE_CHARS, 'passphrase');
    if (!$('setup-audit-stored').checked) problems.push({ id: 'setup-audit-stored', text: 'Confirm that you stored the passphrase: it cannot be recovered.' });
    if (setupProblems(form, problems)) return;
    try {
      const r = await api('PUT', '/api/admin/v1/setup/audit-key', { passphrase: $('setup-audit').value, confirm: $('setup-audit2').value });
      clearSecrets(['setup-audit', 'setup-audit2']);
      $('setup-audit-stored').checked = false;
      $('setup-audit-keyid').textContent = r.key_id;
      show($('setup-audit-form'), false);
      show($('setup-audit-done'), true);
      setStep(2, true);
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
  });
  on('setup-audit-continue', 'click', () => route());

  on('setup-password-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('setup-password-form-error');
    show(err, false);
    if (setupProblems(e.target, ui.pairProblems('setup-password', K.MIN_PASSWORD_CHARS, 'password'))) return;
    try {
      await api('PUT', '/api/admin/v1/password', { current: $('setup-current').value, new: $('setup-password').value });
      $('setup-current').value = '';
      clearSecrets(['setup-password', 'setup-password2']);
      route();
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
  });

  // The first-run hint (plan step 6.3) says the published password is still in use. The signed-out page reads it from the
  // public setup status, once per sign-in screen; a request already in flight is not repeated (api() also shows this
  // screen on a 401, right before route() does). A failed read leaves the hint off.
  let hintRequest = null;
  function loadLoginHint() {
    if (hintRequest) return;
    hintRequest = api('GET', '/api/admin/v1/setup')
      .then((s) => show($('login-hint'), !!s.password_change_required))
      .catch(() => show($('login-hint'), false))
      .finally(() => { hintRequest = null; });
  }

  function showLogin() {
    hideAll();
    show($('login'), true);
    show($('login-hint'), false);
    $('password').value = '';
    loadLoginHint();
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
    { key: 'no-user', test: (d) => d.users && d.users.length === 0, text: 'No user exists yet, so there is nobody to pair an app for.', fix: 'Add user', href: '/admin/people/#add-user' },
    { key: 'no-client', test: (d) => d.users && d.users.length > 0 && d.clients && d.clients.length === 0, text: 'No client is paired yet.', fix: 'Pair a client', href: '/admin/people/#pair-open' },
    { key: 'no-backup-key', test: (d) => d.schedule && !d.schedule.enabled, text: 'No backup key is set, so nothing is backed up.', fix: 'Open Security', href: '/admin/security/#bk-pass' },
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
  // The range of the figures and the chart, in days: one of HOME_STATS_RANGE_DAYS, from the address (?range=) or the default.
  const rangeParam = () => Number(new URLSearchParams(location.search).get('range'));
  let rangeDays = null; // set on first use, once the constants are here
  const range = () => { if (rangeDays === null) rangeDays = K.HOME_STATS_RANGE_DAYS.includes(rangeParam()) ? rangeParam() : K.HOME_STATS_WINDOW_DAYS; return rangeDays; };
  const rangeWords = (days) => (days === 1 ? 'last ' + days * 24 + ' hours' : 'last ' + days + ' days');
  const homeReads = () => ({
    ...GAP_READS,
    stats: '/api/admin/v1/audit/stats?since=' + (Math.floor(Date.now() / MS) - range() * ui.DAY),
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

  // The one reader of GET /audit/stats on Home (audit_calls.js reads the same document for its figures; it is not loaded here).
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

  // A figure with its unit in smaller muted type after it, so "1,280,000 tokens" stays on one line.
  const figure = (n, unit) => ui.el('span', { class: 'num' }, Number(n).toLocaleString(), unit ? ui.el('span', { class: 'unit', text: ' ' + unit }) : null);
  function renderKpis(f) {
    const put = (id, node) => $(id).replaceChildren(node);
    put('kpi-requests', f.requests == null ? dash() : ui.num(f.requests));
    put('kpi-errors', f.errorRate == null ? dash() : figure(f.errorRate, '%'));
    put('kpi-tokens', f.tokens == null ? dash() : figure(f.tokens, 'tokens'));
    put('kpi-cost', f.cost != null ? ui.num(f.cost) : ui.el('span', { class: 'num', text: f.costUnknown ? 'unknown' : '–' }));
    put('kpi-latency', latencyNum(f.p95));
  }

  // The chart is CSS bars (heights set through the CSSOM) in a role="img" whose label is the visible one-line summary; the
  // same values are in a visually hidden table. A read that failed leaves the numbers as they were.
  function renderChart(days) {
    const summary = $('chart-summary');
    const chart = $('home-chart');
    const wrap = $('chart-table-wrap');
    chart.replaceChildren();
    $('chart-table').tBodies[0].replaceChildren();
    if (!days.length) {
      summary.textContent = 'No requests in the ' + rangeWords(range()) + '.';
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
    const today = new Date().toISOString().slice(0, 10);
    for (const d of days) {
      const bar = ui.el('div', { class: 'chart-bar' + (d.day === today ? ' is-today' : ''), 'data-day': d.day, 'data-requests': d.requests }, ui.el('span', { class: 'chart-val', text: fmt(d.requests) }));
      bar.style.height = (d.requests / max * 100) + '%';
      plot.append(ui.el('div', { class: 'chart-col' }, bar));
    }
    const y = ui.el('div', { class: 'chart-y' }, ui.el('span', { text: fmt(max) }), ui.el('span', { text: '0' }));
    y.children[0].style.bottom = '100%';
    y.children[1].style.bottom = '0';
    chart.append(y, plot, ui.el('div', { class: 'chart-x' }, ui.el('span', { text: dayLabel(days[0].day) }), days.length > 1 ? ui.el('span', { text: days[days.length - 1].day === today ? 'Today' : dayLabel(days[days.length - 1].day) }) : null));
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

  // The range's words in the two headings and the select (also called on a change, since an answer equal to the last one is not drawn again).
  function drawRange() {
    $('home-range').value = String(range());
    $('numbers-title').textContent = rangeWords(range()).replace(/^l/, 'L');
    $('usage-title').textContent = 'Requests per day, ' + rangeWords(range());
  }
  function renderHome(d) {
    drawRange();
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
        onError: pageFailed,
      });
    }
    return liveAttention.start();
  }

  // The range selector: the choice goes into the address (?range=, so reload keeps it), the figures and the chart are read
  // again for it, and the change is announced. The select shows the address's range on every draw.
  const rangeSelect = $('home-range');
  if (rangeSelect) {
    rangeSelect.addEventListener('change', () => {
      rangeDays = Number(rangeSelect.value);
      const u = new URL(location.href);
      u.searchParams.set('range', String(rangeDays));
      history.replaceState(history.state, '', u.pathname + u.search + u.hash);
      drawRange();
      ui.announce('Showing the ' + rangeWords(range()));
      if (liveAttention) liveAttention.refresh({ force: true });
    });
  }

  // Quick actions: links to the page that does the work, and Back up now, which is the Backup page's own call.
  on('home-backup', 'click', async (e) => {
    const button = e.currentTarget;
    const done = ui.busy(button, 'Backing up…');
    ui.inlineError('home-error', '');
    try {
      const r = await api('POST', '/api/admin/v1/backup/run');
      ui.toast('Backup made: ' + r.file);
      if (liveAttention) liveAttention.refresh();
    } catch (ex) {
      ui.inlineError('home-error', ex.message);
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
  const listPage = ['services', 'providers', 'people'].includes(page);
  const liveStatus = ui.live(() => api('GET', '/api/admin/v1/status'), {
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
    let text = 'Kill: unknown';
    try {
      const s = await api('GET', '/api/admin/v1/settings');
      state = s.kill_switch ? 'error' : 'ok';
      text = s.kill_switch ? 'Kill: on' : 'Kill: off';
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
  // The health line's state and words, from the service list.
  function healthOf(services) {
    const down = services.filter((s) => s.state !== 'running').length;
    return { state: down ? 'warning' : 'ok', text: down ? down + ' of ' + services.length + ' services not running' : 'All ' + services.length + ' services running' };
  }
  const liveHealth = ui.live(() => api('GET', '/api/admin/v1/services'), {
    when: appShownNow,
    render: ({ services }) => {
      const health = healthOf(services);
      showHealth(health.state, health.text);
    },
    // Signed out, or the admin itself is away: say it is not checked.
    onError: () => showHealth('pending', 'Services not checked'),
  });
  // The reference article's two side lists. The outline keeps its own scroll from one article to the next (kept for the tab,
  // so a long outline is not back at the top after each click) and always shows the current page, without moving the page.
  // "On this page" marks the section being read.
  function referencePage(nav) {
    if (!nav) return;
    const key = 'askbar-ref-nav-scroll';
    try { const saved = Number(sessionStorage.getItem(key)); if (saved > 0) nav.scrollTop = saved; } catch (e) { /* storage may be unavailable */ }
    const cur = nav.querySelector('a[aria-current]');
    if (cur && nav.scrollHeight > nav.clientHeight) {
      const top = cur.offsetTop - nav.offsetTop;
      if (top < nav.scrollTop || top + cur.offsetHeight > nav.scrollTop + nav.clientHeight) nav.scrollTop = Math.max(0, top - nav.clientHeight / 2);
    }
    nav.addEventListener('scroll', () => { try { sessionStorage.setItem(key, String(nav.scrollTop)); } catch (e) { /* ignore */ } }, { passive: true });
    const links = [...document.querySelectorAll('.ref-toc a[href^="#"]')];
    if (!links.length) return;
    const heads = links.map((a) => document.getElementById(decodeURIComponent(a.getAttribute('href').slice(1)))).filter(Boolean);
    let queued = false;
    const mark = () => {
      queued = false;
      const line = parseFloat(getComputedStyle(document.documentElement).scrollPaddingTop) || 0;
      let at = heads[0];
      for (const h of heads) if (h.getBoundingClientRect().top <= line + 1) at = h;
      for (const a of links) {
        if (a.getAttribute('href') === '#' + at.id) a.setAttribute('aria-current', 'location'); else a.removeAttribute('aria-current');
      }
    };
    window.addEventListener('scroll', () => { if (!queued) { queued = true; requestAnimationFrame(mark); } }, { passive: true });
    mark();
  }
  function showApp() {
    hideAll();
    show($('app'), true);
    appShown = true;
    loadPage();
    // On a phone the reference's outline starts closed (CSS shows its toggle only there).
    const refNav = document.querySelector('.ref-nav');
    if (refNav && getComputedStyle(refNav.querySelector('summary')).display !== 'none') refNav.open = false;
    referencePage(refNav);
    focusHash();
    ui.helpFromAddress();
    initSearch();
    liveStatus.start();
    liveHealth.start();
  }

  on('login-form', 'submit', async (e) => {
    e.preventDefault();
    const form = $('login-form');
    ui.formErrors(form, []);
    try {
      await api('POST', '/api/admin/v1/session', { username: $('username').value, password: $('password').value });
      route();
    } catch (ex) {
      // Inline on the password field, and focus moves there (rule 26). A lock-out is said as a pause, not as a count.
      const text = ex.status === 429 ? 'Sign-in is paused. Try again later.' : 'Wrong username or password.';
      ui.formErrors(form, [{ id: 'password', text }], { title: 'Sign-in did not work:' });
    }
  });

  on('logout', 'click', async () => {
    try { await api('DELETE', '/api/admin/v1/session'); } catch (_) { /* already signed out */ }
    route();
  });

  // ---------------------------------------------------------- global search

  // The status bar's search (plan step 3.5, rule 4). It finds, in this order: pages, settings (with the value now),
  // people, clients, providers, models, Audit filters, and last Reference (the reference index, step 7.4).
  // Pages come from the menu itself; everything else is read with GETs on the first focus of the field (and again when
  // the last read is older than LIVE_EVERY_SECS), never on page load. The reference index is one GET per page load,
  // on the first focus or the first typed character. No API is added.
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
    ['s-kill-switch', 'Kill switch', 'security', 'settings', (r) => onOff(r.kill_switch)],
    ['s-kill-aborts', 'Stop running replies when the kill switch goes on', 'security', 'settings', (r) => onOff(r.kill_switch_aborts_running !== false)],
    ['s-retention', 'Keep audit records for', 'security', 'settings', (r) => withUnit(r.audit_retention_days, 'days')],
    ['s-unseal-idle', 'Seal after idle minutes', 'security', 'settings', (r) => withUnit(r.unseal_idle_minutes, 'min')],
    ['s-unseal-total', 'Seal after minutes in all', 'security', 'settings', (r) => withUnit(r.unseal_total_minutes, 'min')],
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
    ['n-challenge-dns', 'How to prove the hostname to Let\'s Encrypt', 'network', 'network', (r) => (r.acme_challenge === 'http' ? 'HTTP-01' : 'DNS-01')],
    ['n-allow', 'Only let these addresses open the admin site', 'network', 'network', (r) => onOff(r.allowlist_enabled)],
    ['n-cidr', 'Allowed addresses and networks', 'network', 'network', (r) => r.admin_cidr || 'none'],
    ['n-staging', 'Use the Let\'s Encrypt staging CA', 'network', 'network', (r) => onOff(r.acme_staging)],
    ['pw-current', 'Change admin password', 'security', null, null],
    ['key-name', 'Admin API key', 'security', 'keys', (r) => (r.length ? r[0].name : 'none')],
    ['sched-keep-files', 'Keep at most this many backups', 'backup', 'schedule', (r) => r.keep_files],
    ['sched-keep-days', 'Delete backups older than this many days', 'backup', 'schedule', (r) => r.keep_days],
    ['bk-pass', 'Backup key', 'security', 'backupKey', (r) => (r.set ? 'fingerprint ' + r.fingerprint : 'not set')],
  ];
  // Model calls keeps its filters in the address (?status=, ?provider=, plan step 6.1), so a row here opens the list already
  // filtered. A status is the auditor's own code (`timed_out`); the row shows its word.
  const AUDIT_STATUSES = [['done', 'done'], ['error', 'error'], ['cancelled', 'cancelled'], ['timed_out', 'timed out'], ['interrupted', 'interrupted']];
  const AUDIT_NOTE = 'Audit / Model calls · filters the list';
  const AUDIT_CALLS = '/admin/audit/calls/';

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
    ['people', 'People', () => (search.data.users || []).map((u) => ({ title: u.name, where: 'Settings / People & clients · users', value: u.paused ? 'paused' : '', href: '/admin/people/#users', icon: 'people' }))],
    ['clients', 'Clients', () => (search.data.clients || []).map((c) => {
      const user = (search.data.users || []).find((u) => u.id === c.user_id);
      return { title: c.name, where: 'Settings / People & clients' + (user ? ' · ' + user.name : ''), value: c.revoked ? 'revoked' : 'active', href: '/admin/people/#clients', icon: 'people' };
    })],
    ['providers', 'Providers', () => (search.data.providers || []).map((p) => ({ title: p.name, where: 'Settings / Providers · ' + p.kind, value: '', href: '/admin/providers/#providers', icon: 'providers' }))],
    ['models', 'Models', () => {
      const c = search.data.catalog || { models: [] };
      const name = (id) => { const p = (search.data.providers || []).find((x) => x.id === id); return p ? p.name : 'a provider that is gone'; };
      return c.models.map((m) => ({ title: m.model, where: 'Settings / Providers · ' + name(m.provider_id), value: c.default === m.provider_id + '/' + m.model ? 'default' : '', href: '/admin/providers/#catalog-approved', icon: 'providers' }));
    }],
    ['audit', 'Audit', () => [
      ...AUDIT_STATUSES.map(([code, word]) => ({ title: 'Status: ' + word, where: AUDIT_NOTE, value: '', href: AUDIT_CALLS + '?status=' + code, icon: 'audit' })),
      ...(search.data.providers || []).map((p) => ({ title: 'Provider: ' + p.name, where: AUDIT_NOTE, value: '', href: AUDIT_CALLS + '?provider=' + encodeURIComponent(p.id), icon: 'audit' })),
    ]],
    // Always last (rule 4). Empty until the reference index has been read; a failed read leaves it empty.
    ['reference', 'Reference', () => referenceRows()],
  ];

  // The reference index (`/admin/reference.json`, step 7.2) is read once per page load, on the first focus of the field or
  // its first typed character, and kept for the page's life. A failed read is not retried: its rows stay out.
  const refIndex = { pages: [], asked: false };
  function loadReference() {
    if (refIndex.asked) return;
    refIndex.asked = true;
    fetch('/admin/reference.json', { credentials: 'same-origin' })
      .then((r) => { if (!r.ok) throw new Error('reference ' + r.status); return r.json(); })
      .then((pages) => { refIndex.pages = pages; }, () => { refIndex.pages = []; })
      .then(() => { if (search.combo && document.activeElement === $('gs-input')) search.combo.render(); });
  }

  // One row per page (its title, matched by its summary and keywords too) and one per heading (a topic). A topic's id is
  // `<page>#<heading>`, the same as a help topic; the page's own row is `<page>`. The index page is not a result.
  function referenceRows() {
    const rows = [];
    for (const p of refIndex.pages) {
      if (p.id === 'index') continue;
      const area = 'Reference / ' + p.group;
      rows.push({ title: p.title, where: area, match: [p.summary, ...p.keywords, ...p.settings].join(' '), href: '/admin/reference/' + p.id + '/', topic: p.id, icon: 'reference' });
      for (const h of p.headings) {
        rows.push({ title: h.text, where: area + ' / ' + p.title, match: '', href: '/admin/reference/' + p.id + '/#' + h.id, topic: p.id + '#' + h.id, icon: 'reference' });
      }
    }
    return rows;
  }

  function searchGroups(query) {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    return SEARCH_GROUPS.map(([key, label, items]) => ({
      key,
      label,
      items: items().filter((i) => { const hay = (i.title + ' ' + i.where + ' ' + (i.match || '')).toLowerCase(); return words.every((w) => hay.includes(w)); }).slice(0, K.SEARCH_RESULTS_PER_GROUP),
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

  // A topic that the current page carries its help text for opens the side panel (step 7.3); any other opens its address.
  function pageHasTopic(topic) {
    return [...document.querySelectorAll('template[data-help-topic]')].some((t) => t.dataset.helpTopic === topic);
  }

  function openResult(item) {
    const url = new URL(item.href, location.href);
    $('gs-input').value = '';
    search.combo.render();
    if (item.topic && pageHasTopic(item.topic)) {
      if (onPhone() && document.documentElement.classList.contains('menu-open')) $('menu-open').click();
      ui.help(item.topic);
      return;
    }
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
    input.addEventListener('focus', () => { loadSearchData(); loadReference(); });
    input.addEventListener('input', loadReference);
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
    server: { title: 'Server', does: 'Chats, history and provider calls', warn: 'Replies being written end as interrupted, and apps say "server offline" until it is back (under a minute).' },
    alerter: { title: 'Alerter', does: 'Delivers alerts', warn: 'Alerts wait or are lost for a few seconds, and the admin gives it its delivery settings again.' },
    'model-gateway': { title: 'Model gateway', does: 'Provider calls and provider keys', warn: 'Its provider keys leave its memory, and the admin gives them back within seconds.' },
    admin: { title: 'Admin', does: 'This site', warn: 'This page stops answering for a few seconds, then reloads, and you sign in again.' },
    caddy: { title: 'Caddy (HTTPS front end)', does: 'HTTPS front end', warn: 'Its configuration is pushed again; connections may drop for a moment.' },
    cpa: { title: 'CLIProxyAPI', does: 'Claude and ChatGPT subscriptions', warn: 'Providers that use your Claude or ChatGPT login fail for a few seconds while it starts again.' },
  };
  let servicesBusy = false;

  // One row for each service: its name, what it does, its state (word and symbol) and Restart. Restart opens the
  // question under the row (ui.rowConfirm); the row's own button hides until it closes.
  function serviceRow(svc) {
    const info = SERVICE_INFO[svc.name] || { title: svc.name, does: '', warn: '' };
    const word = svc.state.charAt(0).toUpperCase() + svc.state.slice(1);
    const detail = [svc.health, svc.status].filter(Boolean).join(' · ');
    const restart = ui.el('button', {
      type: 'button', class: 'btn btn-danger btn-s', 'data-action': 'restart', text: 'Restart',
      'aria-label': 'Restart ' + info.title, disabled: servicesBusy,
    });
    const row = ui.el('tr', { 'data-name': svc.name },
      ui.el('td', null, ui.el('strong', { class: 'name', text: info.title }), ui.el('div', { class: 'sub only-phone', text: info.does })),
      ui.el('td', { class: 'col-does', text: info.does }),
      ui.el('td', null, ui.status(svc.state === 'running' ? 'ok' : 'warning', word), detail ? ui.el('div', { class: 'sub', text: detail }) : null),
      ui.el('td', { class: 'actions' }, restart));
    restart.addEventListener('click', () => ui.rowConfirm(row, {
      text: 'Restart ' + info.title + '? ' + info.warn, action: 'Restart', busy: 'Restarting…', trigger: restart,
      onConfirm: () => restartService(svc.name),
    }));
    return row;
  }

  // The restart itself; a failure shows in the page error line. The list is read again once the question has closed.
  async function restartService(name) {
    servicesBusy = true;
    for (const b of document.querySelectorAll('#services-table button[data-action]')) b.disabled = true;
    pageError('');
    try {
      await api('POST', '/api/admin/v1/services/' + encodeURIComponent(name) + '/restart');
      if (name === 'admin') {
        // It exits after answering; wait until it answers again, then start the page afresh.
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

  // The service rows, kept current every STATUS_EVERY_SECS while the tab is visible, and paused while a restart runs.
  // A row with its question open is held as it is (ui.rows), so a refresh never closes it.
  const liveServices = ui.live(() => api('GET', '/api/admin/v1/services'), {
    every: 'STATUS_EVERY_SECS',
    when: appShownNow,
    skip: () => servicesBusy,
    notice: $('services-table'),
    render(data, ctx) {
      ui.rows($('services'), data.services, serviceRow, { key: (svc) => svc.name, force: ctx.force });
      const down = data.services.filter((s) => s.state !== 'running').length;
      $('services-summary').textContent = down ? down + ' of ' + data.services.length + ' not running' : 'All running';
    },
    onError: pageFailed,
  });
  // The egress proxy's recent denied attempts (the admin's last read of its status). A host is attacker-influenced: text only.
  const DENIED_REASONS = {
    not_listed: 'Not on the allowlist', bad_host: 'Not a valid host name', ip_literal: 'An IP address, not a name',
    port: 'Port not allowed', private_address: 'Private address', plain_http: 'Plain HTTP not allowed',
  };
  const liveDenied = ui.live(() => api('GET', '/api/admin/v1/egress/denied'), {
    every: 'STATUS_EVERY_SECS',
    when: appShownNow,
    render(data, ctx) {
      // A proxy that does not answer is said so; an empty list would claim nothing was blocked.
      const note = $('denied-note');
      note.textContent = data.online ? '' : (data.error || 'The egress proxy does not answer.') + ' Nothing leaves the stack until it does.';
      note.hidden = data.online;
      $('denied-table').hidden = !data.online;
      ui.rows($('denied'), data.online ? data.denied : [], (d) => ui.el('tr', null,
        ui.el('td', null, ui.time(d.at)), ui.el('td', { text: d.source }), ui.el('td', { text: d.host }),
        ui.el('td', { text: d.port == null ? '' : String(d.port) }), ui.el('td', { text: DENIED_REASONS[d.reason] || d.reason })),
        { key: (d) => d.source + '|' + d.host + '|' + d.port + '|' + d.reason + '|' + d.at, force: ctx.force, empty: 'Nothing has been blocked.' });
    },
    onError: pageFailed,
  });
  function loadServices() { liveDenied.start(); return liveServices.start(); }
  // ------------------------------------------------------------------ MCP page
  // Every value from a service (outcomes, hosts, errors) is written with textContent only.
  const MCP_TOOLS = ['search', 'fetch'];
  let mcpBusy = false;
  async function setMcpTool(user, held, tool, on) {
    mcpBusy = true;
    pageError('');
    const allowed = (held.allowed || []).filter((t) => t !== tool);
    if (on) allowed.push(tool);
    try { await api('PUT', '/api/admin/v1/mcp/permissions/' + encodeURIComponent(user.id), { allowed, denied: held.denied || [] }); } catch (ex) { pageError(ex.message); }
    mcpBusy = false;
    liveMcp.refresh({ force: true });
  }
  function mcpPermRow(u, perms) {
    const held = perms[u.id] || { allowed: [], denied: [] };
    return ui.el('tr', { 'data-user': u.id },
      ui.el('td', { text: u.name }),
      ...MCP_TOOLS.map((tool) => {
        const denied = (held.denied || []).includes(tool);
        const box = ui.el('input', { type: 'checkbox', 'aria-label': u.name + ': ' + tool, disabled: denied });
        box.checked = (held.allowed || []).includes(tool);
        box.addEventListener('change', () => setMcpTool(u, held, tool, box.checked));
        return ui.el('td', null, ui.el('label', { class: 'check' }, box, denied ? ' Denied' : box.checked ? ' On' : ' Off'));
      }));
  }
  const mcpDeniedTotal = (d) => Object.values(d || {}).reduce((a, b) => a + b, 0);
  const mcpEntry = (e) => e.any_public ? 'any public host, port ' + e.port : e.host + ':' + e.port + (e.plain_http ? ' (plain HTTP)' : '') + (e.relay ? ' (mail relay)' : '');
  const liveMcp = ui.live(() => Promise.all([
    api('GET', '/api/admin/v1/users'), api('GET', '/api/admin/v1/mcp/permissions'),
    api('GET', '/api/admin/v1/mcp/status'), api('GET', '/api/admin/v1/egress/status'),
  ]), {
    every: 'STATUS_EVERY_SECS',
    when: appShownNow,
    skip: () => mcpBusy,
    render([users, perms, st, eg], ctx) {
      ui.rows($('mcp-perms'), users, (u) => mcpPermRow(u, perms), {
        key: (u) => u.id + '|' + JSON.stringify(perms[u.id] || null), force: ctx.force, empty: 'No people yet.',
      });
      const banner = $('mcp-banner');
      banner.textContent = '';
      if (!st.configured) {
        banner.appendChild(ui.notice('warning', 'The MCP front has no configuration, so it refuses every call.',
          st.online ? 'The admin pushes it within seconds.' : (st.error || 'The MCP front does not answer.')));
      }
      $('mcp-front-state').replaceChildren(ui.status(st.online ? 'ok' : 'warning', st.online ? 'Answering' : 'Not answering'));
      $('mcp-front-versions').textContent = 'Pushed version ' + st.pushed_version + ' · reported version ' + (st.reported_version == null ? 'none' : st.reported_version) + (st.error ? ' · ' + st.error : '');
      ui.rows($('mcp-calls'), Object.entries(st.calls), ([k, n]) => ui.el('tr', null, ui.el('td', { text: k }), ui.el('td', { class: 'num', text: String(n) })),
        { key: ([k, n]) => k + '|' + n, force: ctx.force, empty: 'No call yet.' });
      $('mcp-fingerprint').textContent = st.approval_key ? st.approval_key.fingerprint : 'none';
      $('mcp-key-version').textContent = st.approval_key ? String(st.approval_key.version) : '0';
      const note = $('mcp-egress-note');
      note.textContent = eg.online ? '' : (eg.error || 'The egress proxy does not answer.') + ' The counts are from its last answer.';
      note.hidden = eg.online;
      const rows = ['searxng', 'fetch'].filter((n) => eg.sources[n]).map((n) => [n, eg.sources[n]]);
      ui.rows($('mcp-egress'), rows, ([n, s]) => ui.el('tr', null,
        ui.el('td', { text: n }), ui.el('td', { text: s.entries.map(mcpEntry).join(', ') || 'none' }),
        ui.el('td', { class: 'num', text: String(s.allowed) }), ui.el('td', { class: 'num', text: String(mcpDeniedTotal(s.denied)) })),
        { key: ([n, s]) => n + '|' + JSON.stringify(s), force: ctx.force, empty: 'The egress proxy reports neither source yet.' });
    },
    onError: pageFailed,
  });
  on('mcp-rotate', 'click', async () => {
    const sure = await ui.confirm({ title: 'Rotate the approval key?', body: 'Approvals signed with the old key stop working at once. It needs a sign-in within the last 10 minutes.', action: 'Rotate' });
    if (!sure) return;
    pageError('');
    try { await api('POST', '/api/admin/v1/mcp/approval-key/rotate'); ui.toast('Approval key rotated.'); } catch (ex) { pageError(ex.message); }
    liveMcp.refresh({ force: true });
  });
  function loadMcp() { return liveMcp.start(); }
  // -------------------------------------------------------- providers and models (plan step 5.5)

  // ---- Claude and ChatGPT accounts (CLIProxyAPI): connect an account without a terminal. The paste step (the
  // address the browser lands on, http://localhost...) shows when no domain is set, or when a login started with a
  // domain did not finish; with a domain set the admin first waits for CLIProxyAPI to confirm the login on its own.
  let login = null; // { state, alive } while a login is open

  function cpaMessage(text) { $('cpa-message').textContent = text || ''; }

  function closeLogin() {
    if (login) login.alive = false;
    login = null;
    $('cpa-redirect').value = '';
    show($('cpa-paste'), false);
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
    if (!c.reachable) cpaMessage('CLIProxyAPI is not answering. Check it on the Services page.');
    for (const a of c.accounts) {
      box.appendChild(ui.el('div', { class: 'row', 'data-account': a.name },
        ui.el('span', { text: (a.provider === 'anthropic' ? 'Claude' : 'ChatGPT') + ' · ' + a.label }),
        ui.el('span', { class: 'spacer' }),
        ui.el('button', {
          type: 'button', class: 'btn btn-danger btn-s', 'data-action': 'disconnect', text: 'Disconnect', 'aria-label': 'Disconnect ' + a.label,
          onclick: async () => {
            const sure = await ui.confirm({ title: 'Disconnect ' + a.label + '?', body: 'Replies through the CPA provider stop working until you connect again.', action: 'Disconnect' });
            if (!sure) return;
            try { await api('DELETE', '/api/admin/v1/cpa/accounts/' + encodeURIComponent(a.name)); } catch (ex) { pageError(ex.message); }
            loadCpa();
          },
        })));
    }
    if (!c.accounts.length && c.reachable) {
      box.appendChild(ui.el('p', { class: 'cpa-empty', text: 'No account connected, so the CPA provider cannot answer yet.' }));
    }
  }

  async function connect(provider) {
    cpaMessage('');
    let domain = '';
    // Unknown counts as no domain: the paste step is the safe fallback.
    try { domain = String((await api('GET', '/api/admin/v1/network')).hostname || '').trim(); } catch (_) { /* shown below if the login cannot start */ }
    try {
      const r = await api('POST', '/api/admin/v1/cpa/logins', { provider });
      closeLogin();
      login = { state: r.state, alive: true };
      const a = $('cpa-login-link');
      a.textContent = r.url;
      a.href = r.url;
      show($('cpa-paste'), !domain);
      show($('cpa-login'), true);
      (domain ? a : $('cpa-redirect')).focus();
      watchLogin(login);
    } catch (ex) { cpaMessage(ex.message); }
  }
  on('cpa-connect-anthropic', 'click', () => connect('anthropic'));
  on('cpa-connect-openai', 'click', () => connect('openai'));
  on('cpa-cancel', 'click', () => { closeLogin(); cpaMessage(''); });

  // Asks CLIProxyAPI every CPA_LOGIN_POLL_SECS whether the login finished, up to CPA_LOGIN_POLLS times. A login that
  // fails or never confirms brings the paste step back, as the way to finish by hand.
  async function watchLogin(l) {
    const unfinished = (text) => { cpaMessage(text); show($('cpa-paste'), true); };
    for (let i = 0; i < K.CPA_LOGIN_POLLS; i++) {
      await new Promise((res) => setTimeout(res, K.CPA_LOGIN_POLL_SECS * MS));
      if (!l.alive) return;
      let r;
      try { r = await api('GET', '/api/admin/v1/cpa/logins/' + encodeURIComponent(l.state)); } catch (ex) { if (l.alive) cpaMessage(ex.message); return; }
      if (!l.alive) return;
      if (r.status === 'ok') {
        closeLogin();
        ui.toast('Connected.');
        cpaMessage(r.provider_id ? 'Connected. Pick the test model on the CPA provider.' : '');
        loadCpa();
        liveProviders.refresh();
        return;
      }
      if (r.status === 'error') { unfinished('The sign-in did not finish' + (r.message ? ': ' + r.message : '.')); return; }
    }
    unfinished('CLIProxyAPI has not confirmed the sign-in yet.');
  }

  on('cpa-finish', 'click', async () => {
    if (!login) return;
    const l = login;
    const done = ui.busy($('cpa-finish'), 'Sending…');
    try {
      await api('POST', '/api/admin/v1/cpa/logins/' + encodeURIComponent(l.state) + '/callback', { redirect_url: $('cpa-redirect').value.trim() });
    } catch (ex) { cpaMessage(ex.message); return; } finally { done(); }
    cpaMessage('Waiting for CLIProxyAPI to confirm…');
    l.alive = false;
    login = { state: l.state, alive: true };
    watchLogin(login);
  });

  // ---- the providers table

  const PROVIDER_PANEL = 'provider-panel';
  let editing = null; // the provider being edited, or null when adding
  let editingHasKey = false;
  let providerList = [];
  let providersById = new Map();
  const testResults = new Map(); // provider id -> { testing } | { ok, message, details }

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

  function button(label, cls, onClick) {
    const b = document.createElement('button');
    b.type = 'button';
    if (cls) b.className = cls;
    b.textContent = label;
    b.addEventListener('click', onClick);
    return b;
  }

  const approvedCount = (id) => catalogSaved.models.filter((m) => m.provider_id === id).length;

  function providerRow(p) {
    const r = testResults.get(p.id);
    const sub = [p.managed ? 'managed by the admin' : null, p.key_missing ? 'key missing on the server, enter it again' : p.has_key ? 'key set' : 'no key'].filter(Boolean).join(' · ');
    const result = ui.el('div', { class: 'test-result' });
    if (!r) result.append(ui.el('span', { class: 'hint', 'aria-hidden': 'true', text: '—' }), ui.el('span', { class: 'visually-hidden', text: 'Not tested' }));
    else if (r.testing) result.append(ui.status('pending', 'Testing…'));
    else {
      result.append(ui.status(r.ok ? 'ok' : 'error', r.ok ? 'OK' : 'Failed'), ' ', ui.el('span', { class: 'test-line', text: r.message }));
      if (r.details) result.append(testDetails(r.details));
    }
    const n = approvedCount(p.id);
    return ui.el('tr', { 'data-name': p.name },
      ui.el('td', null, ui.el('strong', { class: 'name', text: p.name }), ui.el('div', { class: 'sub', text: sub })),
      ui.el('td', { class: 'col-type' }, ui.el('code', { text: p.type })),
      ui.el('td', null, result),
      ui.el('td', { class: 'col-models num' }, n ? ui.el('span', { text: n + ' approved' }) : ui.el('span', { class: 'hint', 'aria-hidden': 'true', text: '—' }), n ? null : ui.el('span', { class: 'visually-hidden', text: 'None approved' })),
      ui.el('td', { class: 'actions' }, ui.el('div', { class: 'row-actions' },
        ui.el('button', { type: 'button', class: 'btn btn-secondary btn-s', 'data-action': 'test', text: 'Test', 'aria-label': 'Test ' + p.name, 'aria-disabled': r && r.testing ? 'true' : null, onclick: () => testProvider(p) }),
        ui.el('button', { type: 'button', class: 'btn btn-secondary btn-s', 'data-action': 'edit', text: 'Edit', 'aria-label': 'Edit ' + p.name, onclick: () => openForm(p) }))));
  }

  function renderProviders(opts = {}) {
    ui.rows($('providers'), providerList, providerRow, {
      sig: (p) => JSON.stringify([p, testResults.get(p.id) || null, approvedCount(p.id)]),
      fresh: !!opts.fresh, force: !!opts.force, noun: ['provider', 'providers'],
      empty: 'No providers yet. Add one so clients have something to talk to.',
    });
  }

  async function testProvider(p) {
    const now = testResults.get(p.id);
    if (now && now.testing) return;
    testResults.set(p.id, { testing: true });
    renderProviders();
    let r;
    try {
      const res = await api('POST', '/api/admin/v1/providers/' + encodeURIComponent(p.id) + '/test');
      r = { ok: !!res.ok, message: res.message, details: res.details || null };
    } catch (ex) {
      r = { ok: false, message: ex.message, details: null };
    }
    testResults.set(p.id, r);
    renderProviders();
    ui.announce(p.name + ': test ' + (r.ok ? 'passed' : 'failed'));
  }

  // The provider rows are current every LIVE_EVERY_SECS while the tab is visible. The approved-models draft below is not
  // touched by a check: it is read when the page opens and after the admin's own change.
  const liveProviders = ui.live(() => api('GET', '/api/admin/v1/providers'), {
    when: appShownNow,
    notice: $('providers-table') ? $('providers-table').parentElement : null,
    render(list, ctx) {
      providerList = list;
      providersById = new Map(list.map((p) => [p.id, p]));
      for (const id of [...testResults.keys()]) if (!providersById.has(id)) testResults.delete(id);
      renderProviders({ fresh: !ctx.first, force: ctx.force });
      syncPicker();
    },
    onError: pageFailed,
  });

  let providersReady = false;
  async function loadProviders() {
    if (!providersReady) { providersReady = true; initProvidersPage(); }
    openFromAddress();
    await liveProviders.start();
    await loadCatalog();
    loadRoles();
    loadCpa();
  }

  // ---- the model picker (ADR 0014): the models people may use, and the default. A draft is edited here and saved whole.
  let catalogSaved = { models: [], default: null };
  let catalogDraft = { models: [], default: null };
  let catalogTracker = null;
  let catalogFilterProvider = ''; // '' is every provider
  const offered = new Map(); // provider id -> { state: 'loading' | 'ok' | 'error', models: [name], message }

  const modelId = (m) => m.provider_id + '/' + m.model;
  const providerName = (id) => (providersById.get(id) ? providersById.get(id).name : 'a provider that is gone');
  const catalogSig = (c) => JSON.stringify([c.models.map(modelId).sort(), c.default]);
  const cloneCatalog = (c) => ({ models: c.models.map((m) => ({ provider_id: m.provider_id, model: m.model })), default: c.default });

  async function loadCatalog() {
    catalogSaved = await api('GET', '/api/admin/v1/catalog');
    catalogDraft = cloneCatalog(catalogSaved);
    $('catalog-state').value = catalogSig(catalogDraft);
    if (catalogTracker) catalogTracker.saved();
    show($('catalog-error'), false);
    renderPicker();
    renderProviders();
    renderRoles();
  }

  // ---- the model for each job (plan W4.2): one approved model pinned per role, saved a row at a time.
  const ROLE_NAMES = { 'chat.manual': 'Chat', 'search.query': 'Search: write the query', 'search.writer': 'Search: write the answer' };
  let roleRows = [];

  async function loadRoles() {
    roleRows = await api('GET', '/api/admin/v1/roles');
    renderRoles();
  }

  async function saveRole(role, pin) {
    const err = $('roles-error');
    show(err, false);
    try {
      await (pin ? api('PUT', '/api/admin/v1/roles/' + role, pin) : api('DELETE', '/api/admin/v1/roles/' + role));
      ui.toast(pin ? 'Model pinned.' : 'Pin cleared.');
      await loadRoles();
    } catch (ex) { ui.inlineError(err, ex.message); }
  }

  function renderRoles() {
    const options = catalogSaved.models.map((m) => ui.el('option', { value: modelId(m), text: m.model + ' · ' + providerName(m.provider_id) }));
    $('roles').replaceChildren(...roleRows.map((r) => {
      const id = r.pin ? modelId(r.pin) : '';
      const pick = ui.el('select', { 'aria-label': 'Model for ' + (ROLE_NAMES[r.role] || r.role), disabled: !options.length }, ui.el('option', { value: '', text: 'Choose a model' }), ...options.map((o) => o.cloneNode(true)));
      pick.value = id;
      const shown = !r.pin ? ui.el('span', { class: 'hint', text: 'None' })
        : ui.el('span', null, ui.el('code', { text: r.pin.model }), ' ', ui.el('small', { class: 'hint', text: providerName(r.pin.provider_id) + (r.approved ? '' : ' · Not approved') }));
      const save = ui.el('button', { type: 'button', class: 'btn btn-secondary btn-s', text: 'Save', 'aria-label': 'Save ' + r.role, onclick: () => {
        const m = catalogSaved.models.find((x) => modelId(x) === pick.value);
        if (m) saveRole(r.role, { provider_id: m.provider_id, model: m.model });
      } });
      const clear = ui.el('button', { type: 'button', class: 'btn btn-secondary btn-s', text: 'Clear', 'aria-label': 'Clear ' + r.role, disabled: !r.pin, onclick: () => saveRole(r.role) });
      return ui.el('tr', { 'data-role': r.role },
        ui.el('td', null, ui.el('strong', { text: ROLE_NAMES[r.role] || r.role }), ui.el('div', { class: 'sub' }, ui.el('code', { text: r.role }))),
        ui.el('td', null, shown), ui.el('td', null, pick),
        ui.el('td', { class: 'actions' }, ui.el('div', { class: 'row-actions' }, save, clear)));
    }));
  }

  // The save bar text says what changed in the draft, since the tracked field is only a summary of it.
  function describeDraft() {
    const was = new Set(catalogSaved.models.map(modelId));
    const now = new Set(catalogDraft.models.map(modelId));
    const added = [...now].filter((id) => !was.has(id)).length;
    const removed = [...was].filter((id) => !now.has(id)).length;
    const parts = [];
    if (added) parts.push(added + ' added');
    if (removed) parts.push(removed + ' removed');
    if (catalogDraft.default !== catalogSaved.default && catalogDraft.models.length) parts.push('default changed');
    return parts.length ? 'Unsaved changes: ' + parts.join(', ') : null;
  }

  function draftChanged() {
    renderPicker();
    $('catalog-state').value = catalogSig(catalogDraft);
    $('catalog-form').dispatchEvent(new Event('input'));
    const text = describeDraft();
    const line = $('catalog-form-save-bar').querySelector('[data-save-text]');
    if (text && line) line.textContent = text;
  }

  function setApproved(m, approved) {
    const id = modelId(m);
    const has = catalogDraft.models.some((x) => modelId(x) === id);
    if (approved && !has) catalogDraft.models.push({ provider_id: m.provider_id, model: m.model });
    if (!approved && has) catalogDraft.models = catalogDraft.models.filter((x) => modelId(x) !== id);
    // One default whenever anything is approved: the first one if there was none (or it was just removed).
    if (!catalogDraft.models.length) catalogDraft.default = null;
    else if (!catalogDraft.models.some((x) => modelId(x) === catalogDraft.default)) catalogDraft.default = modelId(catalogDraft.models[0]);
    draftChanged();
  }

  function offeredRow(p, name) {
    const m = { provider_id: p.id, model: name };
    const id = modelId(m);
    const on = catalogDraft.models.some((x) => modelId(x) === id);
    const isNew = on && !catalogSaved.models.some((x) => modelId(x) === id);
    const box = ui.el('input', { type: 'checkbox', checked: on, 'data-focus': 'opt:' + id, 'aria-label': name + ', ' + p.name });
    box.addEventListener('change', () => setApproved(m, box.checked));
    return ui.el('label', { class: 'check picker-opt' }, box, ui.el('code', { text: name }), isNew ? ui.el('span', { class: 'tag', text: 'Added' }) : null);
  }

  function renderOffered() {
    const q = $('catalog-filter').value.trim().toLowerCase();
    const nodes = [];
    if (!providerList.length) nodes.push(ui.el('p', { class: 'hint', text: 'Add a provider first.' }));
    for (const p of providerList) {
      if (catalogFilterProvider && p.id !== catalogFilterProvider) continue;
      const o = offered.get(p.id) || { state: 'idle', models: [] };
      const names = o.state === 'ok' ? o.models.filter((n) => !q || n.toLowerCase().includes(q)) : [];
      const approved = o.models.filter((n) => catalogDraft.models.some((x) => modelId(x) === p.id + '/' + n)).length;
      nodes.push(ui.el('div', { class: 'picker-group' }, ui.el('span', { text: p.name }), ui.el('span', { class: 'hint', text: o.state === 'ok' ? approved + ' of ' + o.models.length + ' approved' : '' })));
      if (o.state === 'idle') nodes.push(ui.el('p', { class: 'hint', text: 'Not loaded. Press Reload to ask this provider for its models.' }));
      if (o.state === 'loading') nodes.push(ui.el('p', { class: 'hint', text: 'Loading models…' }));
      if (o.state === 'error') {
        nodes.push(ui.el('div', { class: 'picker-fail' }, ui.el('span', { class: 'error', text: o.message }),
          ui.el('button', { type: 'button', class: 'btn btn-secondary btn-s', text: 'Try again', 'aria-label': 'Try again: ' + p.name, 'data-focus': 'retry:' + p.id, onclick: () => loadOffered(p) })));
      }
      for (const n of names) nodes.push(offeredRow(p, n));
      if (o.state === 'ok' && q && !names.length) nodes.push(ui.el('p', { class: 'hint', text: 'No model matches.' }));
    }
    $('catalog-offered').replaceChildren(...nodes);
  }

  function renderApproved() {
    $('catalog-approved-title').textContent = 'Approved for everyone · ' + catalogDraft.models.length;
    if (!catalogDraft.models.length) {
      $('catalog-approved').replaceChildren(ui.el('p', { class: 'hint', text: 'No model is approved yet, so nothing answers. Tick models on the left.' }));
      return;
    }
    $('catalog-approved').replaceChildren(...catalogDraft.models.map((m) => {
      const id = modelId(m);
      const isDef = catalogDraft.default === id;
      const isNew = !catalogSaved.models.some((x) => modelId(x) === id);
      const word = isDef ? 'Default' : 'Make default';
      const radio = ui.el('input', { type: 'radio', name: 'catalog-default', checked: isDef, 'data-focus': 'def:' + id, 'aria-label': word + ': ' + m.model });
      radio.addEventListener('change', () => { catalogDraft.default = id; draftChanged(); });
      return ui.el('div', { class: 'picker-row' + (isDef ? ' is-default' : ''), 'data-model': id },
        ui.el('span', { class: 'who' }, ui.el('code', { text: m.model }), ui.el('small', { class: 'hint', text: providerName(m.provider_id) + (isNew ? ' · Added' : '') })),
        ui.el('label', { class: 'check picker-def' }, radio, ui.el('span', { text: word })),
        ui.el('button', { type: 'button', class: 'btn btn-danger btn-s', text: 'Remove', 'aria-label': 'Remove ' + m.model, 'data-focus': 'rm:' + id, onclick: () => setApproved(m, false) }));
    }));
  }

  // A load that finishes while someone ticks a box redraws the lists; focus goes back to the same control.
  function renderPicker() {
    const active = document.activeElement;
    const key = active && active.dataset ? active.dataset.focus : null;
    renderOffered();
    renderApproved();
    if (key) {
      const again = document.querySelector('[data-focus="' + CSS.escape(key) + '"]');
      if (again && again !== document.activeElement) again.focus();
    }
  }

  function renderSegments() {
    const seg = $('catalog-seg');
    show(seg, providerList.length > 1);
    seg.replaceChildren(...[{ id: '', name: 'All' }, ...providerList].map((p) => ui.el('button', {
      type: 'button', class: 'btn btn-s', 'aria-pressed': String(catalogFilterProvider === p.id), text: p.name,
      onclick: () => { catalogFilterProvider = p.id; renderSegments(); renderPicker(); },
    })));
  }

  async function loadOffered(p) {
    offered.set(p.id, { state: 'loading', models: [] });
    renderPicker();
    let res;
    try { res = await api('POST', '/api/admin/v1/providers/models', { type: p.type, base_url: p.base_url, provider_id: p.id }); }
    catch (ex) { res = { ok: false, message: ex.message }; }
    if (!providersById.has(p.id)) return;
    if (res.ok && res.models.length) offered.set(p.id, { state: 'ok', models: res.models.map((m) => m.id) });
    else offered.set(p.id, { state: 'error', models: [], message: res.ok ? 'The provider listed no models.' : 'Could not load models: ' + (res.message || 'no answer') });
    renderPicker();
  }

  // Called whenever the provider list is drawn: a removed provider's models are dropped. Nothing is asked of a provider
  // until the admin presses Reload (or Try again): opening the page sends no provider key anywhere.
  function syncPicker() {
    for (const id of [...offered.keys()]) if (!providersById.has(id)) offered.delete(id);
    if (catalogFilterProvider && !providersById.has(catalogFilterProvider)) catalogFilterProvider = '';
    renderSegments();
    renderPicker();
  }

  on('catalog-load', 'click', () => {
    for (const p of providerList) if (!catalogFilterProvider || p.id === catalogFilterProvider) loadOffered(p);
  });
  on('catalog-filter', 'input', renderPicker);

  on('catalog-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('catalog-error');
    show(err, false);
    const done = ui.busy($('catalog-form-save-bar').querySelector('[data-save-submit]'), 'Saving…');
    try {
      await api('PUT', '/api/admin/v1/catalog', catalogDraft);
      pageError('');
      await loadCatalog();
      ui.toast('Approved models saved.');
    } catch (ex) {
      ui.inlineError(err, ex.message);
    } finally { done(); }
  });

  // ---- Add and Edit: the docked side panel
  let modelSeq = 0;
  const MODEL_HINT_ADD = 'Tested with a short reply when you add it.';
  const MODEL_HINT_EDIT = 'The model the Test button asks.';

  function canListModels() {
    if ($('p-key').value.trim() !== '' || (editing && editingHasKey)) return true;
    return $('p-type').value === 'openai' && $('p-base-url').value.trim() !== '';
  }

  function fillModelOptions(models) {
    $('p-model-options').replaceChildren(...models.map((m) => ui.el('option', { value: m.id, label: m.label || null })));
  }

  // "Load models" asks the provider (with the key in the form, or the saved one) and offers what it lists under the
  // field; a name can always be typed. Nothing is asked until the button is pressed.
  async function loadModels() {
    const hint = $('p-model-hint');
    if (!canListModels()) { fillModelOptions([]); hint.textContent = 'Enter the address and key first, then load the models.'; return; }
    const seq = ++modelSeq;
    hint.textContent = 'Loading models…';
    const body = { type: $('p-type').value, base_url: $('p-base-url').value.trim() };
    if ($('p-key').value.trim() !== '') body.api_key = $('p-key').value.trim();
    if (editing) body.provider_id = editing.id;
    let res;
    try { res = await api('POST', '/api/admin/v1/providers/models', body); }
    catch (ex) { res = { ok: false, message: ex.message }; }
    if (seq !== modelSeq) return; // a newer request replaced this one
    if (res.ok && res.models.length) {
      fillModelOptions(res.models);
      hint.textContent = res.models.length + ' models available: pick one or type a name.';
    } else {
      fillModelOptions([]);
      hint.textContent = (res.message || 'No models listed.') + ' Type the model name instead.';
    }
  }
  on('p-model-refresh', 'click', loadModels);

  // Opening a panel while one is open closes it first (it asks if there is typed input). Closing goes Back in history,
  // so the next panel waits for that to finish.
  async function openForm(p) {
    if (ui.panelOpen()) {
      const back = history.state && history.state.panel === ui.panelOpen();
      if (!(await ui.closePanel())) return;
      if (back) await new Promise((res) => window.addEventListener('popstate', res, { once: true }));
    }
    editing = p || null;
    editingHasKey = !!(p && p.has_key);
    const v = p || { name: '', type: 'openai', base_url: '', model: '' };
    $('provider-panel-title').textContent = p ? 'Edit ' + p.name : 'Add provider';
    $('p-name').value = v.name || '';
    $('p-type').value = v.type || 'openai';
    $('p-base-url').value = v.base_url || '';
    $('p-model').value = v.model || '';
    // A managed provider (CLIProxyAPI) keeps its name, type, address and key.
    for (const id of ['p-name', 'p-type', 'p-base-url', 'p-key']) $(id).disabled = !!(p && p.managed);
    $('p-key').value = ''; // never prefilled
    $('p-key-hint').textContent = p && p.has_key ? 'Write-only. Empty keeps the current key.' : 'Write-only.';
    $('p-max-tokens').value = v.max_tokens || '';
    $('p-temperature').value = v.temperature == null ? '' : v.temperature;
    $('p-advanced').open = !!(v.max_tokens || v.temperature != null);
    $('p-billing').value = v.billing || 'metered';
    $('p-price-in').value = v.price_in == null ? '' : v.price_in;
    $('p-price-out').value = v.price_out == null ? '' : v.price_out;
    $('p-price-cached').value = v.price_cached_in == null ? '' : v.price_cached_in;
    $('p-model-hint').textContent = p ? MODEL_HINT_EDIT : MODEL_HINT_ADD;
    fillModelOptions([]);
    modelSeq++;
    $('provider-submit').textContent = p ? 'Save' : 'Add and test';
    show($('provider-delete'), !!p && !p.managed);
    show($('provider-error'), false);
    await ui.openPanel(PROVIDER_PANEL, { onClose: () => { editing = null; modelSeq++; $('p-key').value = ''; } });
  }

  // `#add-provider` in the address (Home's quick action, the setup list) opens the Add panel, then leaves the address.
  function openFromAddress() {
    if (location.hash !== '#add-provider') return;
    history.replaceState(null, '', location.pathname + location.search);
    openForm(null);
  }

  function initProvidersPage() {
    for (const id of ['p-name', 'p-model']) $(id).required = true;
    $('p-model').setAttribute('list', 'p-model-options');
    $('p-model').autocomplete = 'off';
    $('p-key').autocomplete = 'off';
    for (const id of ['p-price-in', 'p-price-out', 'p-price-cached', 'p-temperature']) $(id).step = 'any';
    $('p-model-hint').setAttribute('role', 'status');
    catalogTracker = ui.trackChanges($('catalog-form'));
    $('catalog-form-save-bar').querySelector('[data-save-discard]').addEventListener('click', () => {
      catalogDraft = cloneCatalog(catalogSaved);
      renderPicker();
    });
    window.addEventListener('hashchange', openFromAddress);
  }

  on('add-provider', 'click', () => openForm(null));

  on('provider-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('provider-error');
    show(err, false);
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
    const adding = !editing;
    const done = ui.busy($('provider-submit'), adding ? 'Adding…' : 'Saving…');
    try {
      let saved;
      if (adding) saved = await api('POST', '/api/admin/v1/providers', body);
      else saved = await api('PUT', '/api/admin/v1/providers/' + encodeURIComponent(editing.id), body);
      await ui.closePanel({ force: true });
      testResults.delete(saved && saved.id);
      await liveProviders.refresh({ force: true });
      const p = (saved && providersById.get(saved.id)) || providerList.find((x) => x.name === body.name);
      if (p) {
        offered.delete(p.id); // what it offered may have changed; asked again on Reload
        renderPicker();
        if (adding) { ui.announce('Provider added. Testing it now.'); testProvider(p); }
        else ui.toast('Provider saved.');
      }
    } catch (ex) {
      ui.inlineError(err, ex.message);
    } finally { done(); }
  });

  on('provider-delete', 'click', async () => {
    const p = editing;
    if (!p) return;
    const sure = await ui.confirm({ title: 'Delete ' + p.name + '?', body: 'People can no longer use its models. This cannot be undone.', action: 'Delete' });
    if (!sure) return;
    try {
      await api('DELETE', '/api/admin/v1/providers/' + encodeURIComponent(p.id));
    } catch (ex) { err_(ex.message); return; }
    await ui.closePanel({ force: true });
    await liveProviders.refresh({ force: true });
    // The saved list may have lost this provider's models; an unsaved draft is not thrown away for that.
    if (!catalogTracker.dirty()) await loadCatalog();
    ui.toast('Provider deleted.');
  });
  function err_(text) { $('provider-error').textContent = text; show($('provider-error'), true); }

  // --------------------------------------------------------- security (admin password)

  on('password-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('pw-error');
    show(err, false);
    ui.formErrors(e.target, []);
    const problems = ui.pairProblems('pw-new', K.MIN_PASSWORD_CHARS, 'password');
    if (problems.length) return ui.formErrors(e.target, problems);
    try {
      await api('PUT', '/api/admin/v1/password', { current: $('pw-current').value, new: $('pw-new').value });
      for (const id of ['pw-current', 'pw-new', 'pw-new2']) $(id).value = '';
      ui.toast('Password changed. Other browsers have been signed out.');
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
  });

  // ------------------------------------------------------ admin API key

  let keyId = null;
  let revokeArmed = null;

  async function loadKeys() {
    const list = await api('GET', '/api/admin/v1/keys');
    keyId = list.length ? list[0].id : null;
    show($('key-row'), keyId !== null);
    show($('key-form'), keyId === null);
    if (keyId !== null) {
      $('key-row-name').textContent = list[0].name;
      $('key-row-made').replaceChildren(ui.time(list[0].created_at));
      $('key-row-used').replaceChildren(list[0].last_used_at ? ui.time(list[0].last_used_at) : 'Never');
    }
    show($('key-error'), false);
  }

  // The Security page: the keys, the sealed-content and audit settings (own form and save bar), the audit key id, the backup key,
  // the kill switch, and the live length count and mismatch line under the three new-secret pairs.
  let securityWired = false;
  let sealTrack = null;
  let killTrack = null;
  async function loadSecurity() {
    if (!securityWired) {
      securityWired = true;
      ui.pair('pw-new', K.MIN_PASSWORD_CHARS, 'password');
      ui.pair('r-new', K.MIN_PASSPHRASE_CHARS, 'passphrase');
      ui.pair('bk-pass', K.MIN_PASSPHRASE_CHARS, 'passphrase');
    }
    const [, s] = await Promise.all([loadKeys(), api('GET', '/api/admin/v1/settings'), loadBackupKey()]);
    $('s-retention').value = s.audit_retention_days;
    $('s-unseal-idle').value = s.unseal_idle_minutes;
    $('s-unseal-total').value = s.unseal_total_minutes;
    $('s-key-id').textContent = s.audit_key_id || 'none';
    showKill(!!s.kill_switch);
    $('s-kill-aborts').checked = s.kill_switch_aborts_running !== false;
    show($('seal-error'), false);
    show($('kill-error'), false);
    show($('kill-form-error'), false);
    // The first load starts the save bars from these values; later loads (after a save) make them the new base.
    if (sealTrack) sealTrack.saved(); else sealTrack = ui.trackChanges($('seal-form'));
    if (killTrack) killTrack.saved(); else killTrack = ui.trackChanges($('kill-form'));
  }

  on('key-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('key-error');
    show(err, false);
    try {
      const made = await api('POST', '/api/admin/v1/keys', { name: $('key-name').value });
      $('key-name').value = '';
      $('key-value').textContent = made.key; // the only time it is on the page
      ui.openBox('key-dialog');
      await loadKeys();
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
  });

  // Once the box closes (button, Esc) the key is gone from the page; it cannot be read again.
  const clearKey = () => { $('key-value').textContent = ''; };
  on('key-close', 'click', () => { clearKey(); ui.closeBox('key-dialog'); });
  on('key-dialog', 'cancel', clearKey);
  on('key-dialog', 'close', clearKey);

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
      ui.inlineError($('key-error'), ex.message);
    }
  });

  // ------------------------------------------------------- backup key (ADR 0013; its form is on the Security page)

  // The Backup page: the backups, the schedule and Restore (the backup key is on the Security page) (ADR 0013, D27 to D29). Text goes in by textContent.
  // A clock time from seconds after midnight (the run time comes from constants), written as HH:MM.
  const utcClock = (secs) => [Math.floor(secs / 3600), Math.floor((secs % 3600) / 60)].map((n) => String(n).padStart(2, '0')).join(':');
  async function loadBackup() {
    loadRestore();
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
        + (v.missed ? '. No backup has succeeded for over ' + K.BACKUP_MISSED_AFTER_SECS / 3600 + ' hours.' : '');
    $('sched-next').textContent = v.enabled && v.next_run_at ? 'Next: ' + when(v.next_run_at) + ' (' + utcClock(K.BACKUP_RUN_AT_SECS) + ' UTC daily).' : '';
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
    // The first load starts the save bar from these values; later loads (after a save) make them the new base.
    if (scheduleTrack) scheduleTrack.saved(); else scheduleTrack = ui.trackChanges($('schedule-form'));
  }
  let scheduleTrack = null;

  on('sched-run', 'click', async () => {
    const err = $('sched-error');
    show(err, false);
    try {
      const r = await api('POST', '/api/admin/v1/backup/run');
      ui.toast('Backup made: ' + r.file);
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
    loadSchedule().catch((e) => ui.inlineError('sched-error', e.message));
  });

  on('sched-drill', 'click', async () => {
    try { await api('POST', '/api/admin/v1/backup/drill'); await loadSchedule(); } catch (ex) { ui.inlineError('sched-error', ex.message); }
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
      ui.inlineError(err, ex.message);
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
    // The pair and the stored box are checked here, and each problem sits under its own field (rule 26).
    ui.formErrors(e.target, []);
    const pass = $('bk-pass').value;
    const problems = ui.pairProblems('bk-pass', K.MIN_PASSPHRASE_CHARS, 'passphrase');
    if (!$('bk-stored').checked) problems.push({ id: 'bk-stored', text: 'Confirm that you stored the passphrase: it cannot be recovered.' });
    if (problems.length) return ui.formErrors(e.target, problems);
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

  const uploadBundle = (file) => api('POST', '/api/admin/v1/restore/inspect', file, { raw: 'application/x-tar' });

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
      ui.inlineError(err, ex.message);
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
      ui.inlineError(err, ex.message);
    }
  });

  // ----------------------------------------------------------- network

  let networkTrack = null;
  async function loadNetwork() {
    const n = await api('GET', '/api/admin/v1/network');
    $('n-tls').checked = n.tls_enabled;
    $('n-host').value = n.hostname;
    $('n-cidr').value = n.admin_cidr;
    $('n-allow').checked = !!n.allowlist_enabled;
    $('n-staging').checked = n.acme_staging;
    setChallenge(n.acme_challenge || 'dns');
    showChallenge();
    $('n-token').value = '';
    // The token is write-only: the hint says whether one is stored, never what it is (rule 10: no placeholder hints).
    $('n-token-hint').textContent = (n.has_token ? 'Stored and unchanged; type a new one to replace it. ' : '')
      + 'Needs Zone:DNS:Edit and Zone:Zone:Read. Write-only.';
    $('n-clear').checked = false;
    show($('n-clear-row'), n.has_token);
    $('n-https').className = 'status ' + (n.tls_enabled ? 'status-ok' : 'status-off');
    $('n-https-word').textContent = n.tls_enabled ? 'HTTPS on' : 'HTTPS off';
    $('n-info').textContent = (n.tls_enabled && n.hostname ? 'The admin site is at https://' + n.hostname + '/admin. ' : '')
      + 'New pairing keys point at ' + n.public_url + '.';
    showProxy(n.proxy);
    show($('network-error'), false);
    // The first load starts the save bar from these values; later loads (after a save) make them the new base.
    if (networkTrack) networkTrack.saved(); else {
      networkTrack = ui.trackChanges($('network-form'));
      // Discard puts the radios back; the challenge's own part of the form follows them.
      $('network-form-save-bar').querySelector('[data-save-discard]').addEventListener('click', showChallenge);
    }
  }

  // The challenge is a pair of radios (name n-challenge): 'dns' or 'http'.
  function challenge() {
    return $('n-challenge-http').checked ? 'http' : 'dns';
  }
  function setChallenge(value) {
    $('n-challenge-http').checked = value === 'http';
    $('n-challenge-dns').checked = value !== 'http';
  }

  // With HTTP-01 there is no token to enter, and the warning about port 80 shows.
  function showChallenge() {
    const http = challenge() === 'http';
    show($('n-http-note'), http);
    show($('n-dns-box'), !http);
  }
  on('n-challenge-dns', 'change', showChallenge);
  on('n-challenge-http', 'change', showChallenge);

  function showProxy(p) {
    const el = $('n-proxy');
    el.className = p.ok ? 'ok' : (p.at ? 'error' : 'hint');
    el.textContent = 'HTTPS front end: ' + p.message + (p.at ? ' (' + new Date(p.at * 1000).toLocaleTimeString() + ')' : '');
  }

  on('n-apply', 'click', async () => {
    try { showProxy(await api('POST', '/api/admin/v1/network/apply')); } catch (ex) { ui.inlineError('network-error', ex.message); }
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
        acme_challenge: challenge(),
        cloudflare_token: $('n-token').value,
        clear_token: $('n-clear').checked,
      });
      await loadNetwork();
      ui.toast('Network settings saved.');
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
  });

  // ---------------------------------------------------------- people and clients (plan step 5.6)

  function when(ts) {
    return ts ? new Date(ts * 1000).toLocaleString() : 'never';
  }

  // The users, their clients and the pairing box's "Connected": one live read of both lists, every LIVE_EVERY_SECS while the
  // tab is visible and right after the admin's own change (loadUsers, loadClients).
  let userList = [];
  let userNames = {};
  let clientList = [];
  let knownClients = [];
  let editingUser = null;
  let pairing = null; // { user, known: Set of client ids before the key was made, done }
  const liveClients = ui.live(() => Promise.all([
    api('GET', '/api/admin/v1/users'),
    api('GET', '/api/admin/v1/clients'),
  ]), {
    when: appShownNow,
    notice: $('users-section'),
    render([users, clients], ctx) {
      clientList = clients;
      renderUsers(users, ctx);
      renderClients(clients, ctx);
      if (ctx.first) openPeopleFromAddress();
    },
    onError: pageFailed,
  });
  const loadUsers = () => liveClients.refresh();
  function loadClients() { return liveClients.start(); }

  const activeClientsOf = (id) => clientList.filter((d) => d.user_id === id && !d.revoked).length;

  function renderUsers(users, ctx) {
    userList = users;
    userNames = Object.fromEntries(users.map((u) => [u.id, u.name]));
    // One row for each user, with the emergency stop for that person: Pause stops their replies (new and running) until
    // Resume. Everything is set as text.
    ui.rows($('users'), users, userRow, {
      fresh: !ctx.first, force: ctx.force, noun: ['user', 'users'],
      sig: (u) => JSON.stringify([u, activeClientsOf(u.id)]),
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
    syncPairForm();
  }

  function userRow(u) {
    const pauseBtn = ui.el('button', {
      type: 'button', class: 'btn btn-secondary btn-s', 'data-action': u.paused ? 'resume' : 'pause',
      text: u.paused ? 'Resume' : 'Pause', 'aria-label': (u.paused ? 'Resume replies for ' : 'Pause replies for ') + u.name,
      onclick: () => setPaused(u, !u.paused),
    });
    const menu = $('row-menu-template').content.cloneNode(true);
    const more = menu.querySelector('.row-menu-button');
    more.setAttribute('aria-label', 'More actions for ' + u.name);
    more.setAttribute('data-tip', 'More actions');
    const holder = menu.firstElementChild;
    ui.menu(more, (action) => { if (action === 'edit') openEdit(u); else if (action === 'delete') askToDelete(u); });
    return ui.el('tr', { 'data-user': u.id },
      ui.el('td', null, ui.el('strong', { class: 'name', text: u.name })),
      ui.el('td', { class: 'num', text: String(activeClientsOf(u.id)) }),
      ui.el('td', null, u.paused ? ui.status('paused', 'Replies paused') : ui.status('ok', 'Active')),
      ui.el('td', { class: 'actions' }, ui.el('div', { class: 'row-actions' }, pauseBtn, holder)));
  }

  // Pause stops a person's replies, new and running, until Resume. It is the one action here that needs no confirmation.
  async function setPaused(u, paused) {
    show($('user-error'), false);
    try {
      await api('POST', '/api/admin/v1/users/' + encodeURIComponent(u.id) + '/' + (paused ? 'pause' : 'resume'));
      await loadUsers();
      ui.announce(u.name + (paused ? ': replies paused' : ': replies resumed'));
    } catch (ex) {
      ui.inlineError($('user-error'), ex.message);
    }
  }

  // Opening a panel while one is open closes it first (it asks if there is typed input). Closing goes Back in history,
  // so the next panel waits for that to finish.
  async function openEdit(u) {
    if (ui.panelOpen()) {
      const back = history.state && history.state.panel === ui.panelOpen();
      if (!(await ui.closePanel())) return;
      if (back) await new Promise((res) => window.addEventListener('popstate', res, { once: true }));
    }
    editingUser = u;
    $('user-panel-title').textContent = 'Edit ' + u.name;
    $('e-paused').checked = !!u.paused;
    $('edit-delete').setAttribute('aria-label', 'Delete ' + u.name);
    show($('edit-error'), false);
    await ui.openPanel('user-panel', { onClose: () => { editingUser = null; } });
  }

  on('edit-form', 'submit', async (e) => {
    e.preventDefault();
    const u = editingUser;
    if (!u) return;
    show($('edit-error'), false);
    if ($('e-paused').checked === !!u.paused) { await ui.closePanel(); return; }
    const done = ui.busy($('edit-save'), 'Saving…');
    try {
      await api('POST', '/api/admin/v1/users/' + encodeURIComponent(u.id) + '/' + ($('e-paused').checked ? 'pause' : 'resume'));
      await ui.closePanel({ force: true });
      await loadUsers();
      ui.toast(u.name + ($('e-paused').checked ? ': replies paused.' : ': replies resumed.'));
    } catch (ex) {
      ui.inlineError($('edit-error'), ex.message);
    } finally {
      done();
    }
  });
  on('edit-delete', 'click', () => { if (editingUser) askToDelete(editingUser); });

  // Deleting a person removes their personal space, their clients and their sign-ins for good (the audit records stay).
  // The name is typed to confirm, "Pause instead" is the gentler way out, and the server wants a sign-in from the last
  // FRESH_SIGNIN_SECS.
  async function askToDelete(u) {
    const n = activeClientsOf(u.id);
    const sure = await ui.confirm({
      title: 'Delete ' + u.name + '?',
      body: 'This cannot be undone. It removes ' + u.name + "'s personal space, with every conversation in it, signs out " + (n === 1 ? 'their 1 client' : 'their ' + n + ' clients') + ' at once and deletes their pairing keys. Audit records are kept.',
      action: 'Delete ' + u.name,
      typeName: u.name,
      alt: u.paused ? null : { text: 'Might ' + u.name + ' need it back?', label: 'Pause instead', onClick: () => setPaused(u, true) },
    });
    if (!sure) return;
    try {
      await api('DELETE', '/api/admin/v1/users/' + encodeURIComponent(u.id));
      if (ui.panelOpen()) await ui.closePanel({ force: true });
      await loadUsers();
      ui.toast('Deleted ' + u.name + '.');
    } catch (ex) {
      pageError(ex.message);
    }
  }

  // ---- Add user
  function openUserBox() {
    $('u-name').maxLength = K.NAME_MAX_CHARS;
    $('u-name').value = '';
    ui.fieldError('u-name', '');
    show($('user-add-error'), false);
    ui.openBox('user-box');
    $('u-name').focus();
  }
  on('add-user', 'click', openUserBox);
  on('user-cancel', 'click', () => ui.closeBox('user-box'));
  on('user-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('user-add-error');
    show(err, false);
    const name = $('u-name').value.trim();
    if (!name) { ui.fieldError('u-name', 'Enter a name.').focus(); return; }
    ui.fieldError('u-name', '');
    try {
      const r = await api('POST', '/api/admin/v1/users', { name });
      ui.closeBox('user-box');
      await loadUsers();
      $('d-user').value = r.id;
      ui.toast('Added ' + name + '.');
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
  });

  // ---- Paired clients
  function renderClients(list, ctx) {
    knownClients = list;
    ui.rows($('clients'), list, clientRow, {
      fresh: !ctx.first, force: ctx.force, noun: ['client', 'clients'], empty: 'No clients are paired yet.',
      sig: (d) => JSON.stringify([d, userNames[d.user_id]]),
    });
    // The pairing box: the key is used once, so a client that was not there when it was made, for that user, is the app.
    if (pairing && !pairing.done) {
      const hit = list.find((d) => !pairing.known.has(d.id) && d.user_id === pairing.user);
      if (hit) {
        pairing.done = true;
        if ($('pair-box').open) showConnected(hit.name);
        else ui.toast(hit.name + ' is paired.');
      }
    }
  }

  function clientRow(d) {
    const revoke = ui.el('button', {
      type: 'button', class: 'btn btn-danger btn-s', 'data-action': 'revoke', text: 'Revoke', 'aria-label': 'Revoke ' + d.name,
      onclick: () => askToRevoke(d),
    });
    return ui.el('tr', { 'data-client': d.id },
      ui.el('td', null, ui.el('strong', { class: 'name', text: d.name }), d.revoked ? ui.el('div', { class: 'sub', text: 'revoked' }) : null),
      ui.el('td', { text: userNames[d.user_id] || 'unknown user' }),
      ui.el('td', { class: 'col-seen' }, d.last_seen ? ui.time(d.last_seen) : ui.el('span', { text: 'never' })),
      ui.el('td', { class: 'actions' }, d.revoked ? ui.status('off', 'Revoked') : revoke));
  }

  async function askToRevoke(d) {
    const sure = await ui.confirm({ title: 'Revoke ' + d.name + '?', body: 'It will be signed out and must be paired again.', action: 'Revoke' });
    if (!sure) return;
    show($('client-error'), false);
    try {
      await api('POST', '/api/admin/v1/clients/' + encodeURIComponent(d.id) + '/revoke');
      await loadClients();
      ui.toast('Revoked ' + d.name + '.');
    } catch (ex) {
      ui.inlineError($('client-error'), ex.message);
    }
  }

  // ---- The pairing box: the form, then the key (waiting), then Connected when the client shows in the live list.
  // The state line is a word and a symbol (rule 37), read out politely.
  function showPairing(state, word, more) {
    $('pairing-status').replaceChildren(ui.status(state, word), more || '');
  }
  function showConnected(name) {
    show($('pairing-key'), false);
    $('pairing-sum').textContent = 'The pairing key no longer works.';
    showPairing('ok', 'Connected', ' ' + name + ' is paired.');
    $('pairing-again').textContent = 'Pair another client';
    ui.announce(name + ' is paired.');
  }
  function syncPairForm() {
    const none = userList.length === 0;
    $('pair-form').querySelector('button[type=submit]').disabled = none;
    $('pair-error').textContent = none ? 'Add a user first, then pair an app for them.' : '';
    show($('pair-error'), none);
  }
  function resetPairBox() {
    pairing = null;
    $('d-name').value = '';
    $('pairing-string').textContent = '';
    show($('pair-form'), true);
    show($('pairing-out'), false);
    show($('pairing-key'), true);
    $('pairing-again').textContent = 'Make another key';
    syncPairForm();
  }
  function openPairBox() {
    $('d-name').maxLength = K.NAME_MAX_CHARS;
    resetPairBox();
    ui.openBox('pair-box');
    (userList.length ? $('d-user') : $('pair-cancel')).focus();
  }
  on('pair-open', 'click', openPairBox);
  on('pair-cancel', 'click', () => ui.closeBox('pair-box'));
  on('pairing-done', 'click', () => ui.closeBox('pair-box'));
  on('pairing-again', 'click', () => {
    const user = $('d-user').value;
    resetPairBox();
    if (userList.some((u) => u.id === user)) $('d-user').value = user;
    $('d-user').focus();
  });
  on('pair-box', 'close', () => { if (pairing && pairing.done) pairing = null; });

  on('pair-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('pair-error');
    show(err, false);
    try {
      const user = $('d-user').value;
      const name = $('d-name').value.trim();
      const known = new Set(knownClients.map((d) => d.id));
      const r = await api('POST', '/api/admin/v1/users/' + encodeURIComponent(user) + '/pairing-keys', { name });
      $('pairing-string').textContent = r.pairing;
      pairing = { user, known, done: false };
      $('pairing-sum').textContent = 'For ' + (userNames[user] || 'this user') + (name ? ' · client name ' + name : '');
      const mins = ui.constants.PAIR_CODE_TTL_SECS / 60;
      $('pairing-meta').textContent = 'Works once · expires in ' + (Number.isInteger(mins) ? mins + (mins === 1 ? ' minute' : ' minutes') : ui.constants.PAIR_CODE_TTL_SECS + ' seconds') + ' · shown only here';
      showPairing('pending', 'Waiting for the app to pair');
      show($('pair-form'), false);
      show($('pairing-out'), true);
      $('pairing-done').focus();
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
  });

  // `#add-user` and `#pair-open` in the address (Home's quick action, the setup list, search) open their box, then leave
  // the address.
  function openPeopleFromAddress() {
    const hash = location.hash;
    if (hash !== '#add-user' && hash !== '#pair-open') return;
    history.replaceState(null, '', location.pathname + location.search);
    if (hash === '#add-user') openUserBox(); else openPairBox();
  }
  if (page === 'people') window.addEventListener('hashchange', openPeopleFromAddress);

  // --------------------------------------------------------- settings

  // "120 seconds = 2 minutes", "90 seconds = 1.5 minutes".
  function minutesText(secs) {
    if (!Number.isFinite(secs) || secs <= 0) return '';
    const m = secs / 60;
    const shown = Number.isInteger(m) ? String(m) : String(Math.round(m * 10) / 10);
    return secs + ' seconds = ' + shown + (shown === '1' ? ' minute' : ' minutes');
  }
  function showTimeoutMinutes() {
    // The hint already says the default in minutes, so the conversion shows only for another value.
    const secs = parseInt($('s-gen-timeout').value, 10);
    $('s-gen-timeout-min').textContent = secs === K.DEFAULT_GENERATION_TIMEOUT_SECS ? '' : minutesText(secs);
  }
  on('s-gen-timeout', 'input', showTimeoutMinutes);

  // The General page's form (its save bar). The kill switch and the audit settings are on the Security page.
  let settingsTrack = null;
  function showKill(on) {
    $('s-kill-state').textContent = on ? 'on' : 'off';
    $('s-kill-switch').textContent = on ? 'Turn off kill switch' : 'Turn on kill switch';
    $('s-kill-switch').classList.toggle('btn-danger', !on);
    $('s-kill-switch').classList.toggle('btn-secondary', on);
  }
  // The server always sends these when it is read, so the form takes them as they are.
  async function loadSettings() {
    const s = await api('GET', '/api/admin/v1/settings');
    $('s-gen-timeout').value = s.generation_timeout_secs;
    showTimeoutMinutes();
    $('s-history-turns').value = s.history_turns;
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
    // The first load starts the save bar from these values; later loads (after a save) make them the new base.
    if (settingsTrack) settingsTrack.saved(); else settingsTrack = ui.trackChanges($('settings-form'));
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
    // The first load starts each form's save bar from its values; a save makes the new values the base (below).
    if (!alertsTrack) alertsTrack = { ntfy: ui.trackChanges($('alerts-form')), email: ui.trackChanges($('email-form')) };
  }
  let alertsTrack = null;
  // The page-wide banner choices are shown but cannot be stored yet (Not available yet): a click does not change them.
  for (const box of document.querySelectorAll('input[data-na-check]')) box.addEventListener('click', (e) => e.preventDefault());
  function alertsError(ex, id = 'a-error') {
    ui.inlineError($(id), ex.message);
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
      alertsTrack.ntfy.saved();
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
      alertsTrack.email.saved();
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
      if (settingsTrack) settingsTrack.saved();
      ui.toast('Settings saved. Clients pick them up on their next message or when their window opens.');
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
  });

  // The audit and sealed-content settings (Security page): their own save, from a fresh read of the stored settings. Only the
  // three fields change; the prompt and display go back as stored (the server always needs them), so unsaved edits on the General
  // page are never sent from here, and a save of General never sends these three.
  on('seal-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('seal-error');
    show(err, false);
    try {
      const now = await api('GET', '/api/admin/v1/settings');
      await api('PUT', '/api/admin/v1/settings', {
        system_prompt: now.system_prompt,
        display: now.display,
        audit_retention_days: parseInt($('s-retention').value, 10),
        unseal_idle_minutes: parseInt($('s-unseal-idle').value, 10),
        unseal_total_minutes: parseInt($('s-unseal-total').value, 10),
      });
      if (sealTrack) sealTrack.saved();
      ui.toast('Saved.');
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
  });

  // The option "Also stop the replies that are running" (Security page, danger zone) is an ordinary setting with its own save bar.
  // Its save reads the stored settings fresh and sends them back as stored with only this field changed, so unsaved edits in the
  // General page or the sealed-content form are never sent from here.
  on('kill-form', 'submit', async (e) => {
    e.preventDefault();
    const err = $('kill-form-error');
    show(err, false);
    try {
      const now = await api('GET', '/api/admin/v1/settings');
      await api('PUT', '/api/admin/v1/settings', {
        system_prompt: now.system_prompt,
        display: now.display,
        kill_switch_aborts_running: $('s-kill-aborts').checked,
      });
      if (killTrack) killTrack.saved();
      ui.toast('Saved.');
    } catch (ex) {
      ui.inlineError(err, ex.message);
    }
  });

  // The kill switch (Security page, danger zone): asks first (Cancel changes nothing), then saves only itself. The PUT carries
  // the two fields the server always needs plus the SAVED value of the stop-running-replies option from a fresh read; the box
  // is never read here, so an unsaved tick is not used (the confirmation says so). It is an emergency stop: never blocked.
  on('s-kill-switch', 'click', async () => {
    const err = $('kill-error');
    show(err, false);
    try {
      const now = await api('GET', '/api/admin/v1/settings');
      const turningOn = !now.kill_switch;
      const aborts = now.kill_switch_aborts_running !== false;
      const unsaved = !!killTrack && killTrack.dirty();
      const ok = await ui.confirm(turningOn
        ? {
          title: 'Turn on the kill switch?',
          body: 'No new replies start for anyone, and the messages waiting in queues are cleared. '
            + (aborts ? 'Running replies will be stopped.' : 'Running replies will finish.')
            + (unsaved ? ' The unsaved change to "Also stop the replies that are running" is not used until it is saved.' : ''),
          action: 'Turn on kill switch',
          kind: 'danger',
        }
        : { title: 'Turn off the kill switch?', body: 'Replies start again for everyone.', action: 'Turn off kill switch', kind: 'primary' });
      if (!ok) return;
      await api('PUT', '/api/admin/v1/settings', { system_prompt: now.system_prompt, display: now.display, kill_switch: turningOn, kill_switch_aborts_running: aborts });
      showKill(turningOn);
      checkKill();
      ui.toast(turningOn ? 'The kill switch is on. No new replies start.' : 'The kill switch is off. Replies start again.');
    } catch (ex) {
      ui.inlineError(err, ex.message);
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
    ui.formErrors(e.target, []);
    const problems = ui.pairProblems('r-new', K.MIN_PASSPHRASE_CHARS, 'passphrase');
    if (problems.length) return ui.formErrors(e.target, problems);
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
    ui.pair('setup-password', K.MIN_PASSWORD_CHARS, 'password');
    ui.pair('setup-audit', K.MIN_PASSPHRASE_CHARS, 'passphrase');
    ui.pair('setup-backup', K.MIN_PASSPHRASE_CHARS, 'passphrase');
    route();
  })();
})();
