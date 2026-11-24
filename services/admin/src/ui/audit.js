// The Audit pages that read the admin's own records: Sealed access (the session, and who opened sealed content) and
// Routing (the tries of one request, and the providers in a range). One script for both; body[data-page] says which.
// Model calls has its own script (audit_calls.js). Plain JavaScript, no build step. Everything that comes from the
// server (provider names, ids, error codes) goes in through textContent, never as HTML.
(() => {
  'use strict';
  const { api, registerTab, show, $, openSession, closeSession } = window.askbarAdmin;
  const { ui } = window;

  const PAGE = document.body.dataset.page;
  const SEALED = 'audit/sealed';

  // One banner for anything that stops the tab reading: the auditor away (503), or the server away (502), which
  // only provider names need. The views keep their "nothing yet" text underneath.
  const banner = (message) => ui.inlineError('audit-error', message);

  // Runs a read and shows its failure in the banner; returns undefined on failure.
  async function read(path) {
    try {
      const data = await api('GET', path);
      banner('');
      return data;
    } catch (e) {
      if (e.message !== 'Signed out') banner(ui.explain(e));
      return undefined;
    }
  }

  function el(tag, cls, text) {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined) n.textContent = text;
    return n;
  }

  // ------------------------------------------------------------ sealed access

  // The session (ADR 0011): one passphrase, then sealed content stays open until it seals itself, Seal, or sign-out.
  // Nothing opened stays on the page once it is sealed. The server keeps the time; the page counts down.
  const session = { open: false, idleLeft: 0, totalLeft: 0, keys: [] };

  function renderSession() {
    show($('audit-unseal-form'), !session.open);
    show($('audit-open'), session.open);
    if (!session.open) {
      $('audit-pass').value = '';
      return;
    }
    const left = Math.min(session.idleLeft, session.totalLeft);
    const mins = Math.max(1, Math.ceil(left / 60));
    const held = session.keys.filter((k) => k.open).map((k) => k.key_id);
    $('audit-open-state').textContent = 'Unsealed. It seals itself in about ' + mins + ' minute' + (mins === 1 ? '' : 's') + ' (sooner if it is not used; ' + Math.max(1, Math.ceil(session.totalLeft / 60)) + ' at most). Keys open: ' + (held.length ? held.join(', ') : 'none') + '.';
  }

  function applySession(r) {
    session.open = !!r.open;
    session.idleLeft = r.idle_left_secs || 0;
    session.totalLeft = r.total_left_secs || 0;
    session.keys = r.keys || [];
    renderSession();
  }

  // The status poll tells the tab when the session ended on its own (a timeout, a sign-out elsewhere, a restart).
  function sessionFromStatus(s) {
    if (!s.open && session.open) applySession({ open: false });
    else if (s.open && !session.open) loadSession();
  }

  async function loadSession() {
    const r = await read('/api/admin/v1/audit/session');
    if (r) applySession(r);
  }

  // Every attempt to open sealed content, newest first, from the admin's own record. Both results are text.
  async function loadAccess() {
    const r = await read('/api/admin/v1/audit/access');
    if (!r) return;
    show($('audit-access'), r.access.length > 0);
    show($('audit-access-empty'), r.access.length === 0);
    ui.rows($('audit-access-rows'), r.access, (a) => {
      const tr = el('tr');
      tr.dataset.access = String(a.id);
      for (const [text, cls] of [
        [new Date(a.opened_at * 1000).toLocaleString()],
        [a.span_id, 'wrap'],
        [a.key_id],
        [a.ok ? 'opened' : 'wrong passphrase', a.ok ? '' : 'status-error'],
      ]) tr.appendChild(el('td', cls, text));
      // Ended and How it ended are not available yet (rule 40): the cells stay empty and greyed.
      tr.append(el('td', 'na-cell'), el('td', 'na-cell'));
      return tr;
    }, { key: (a) => a.id });
  }

  function startSealed() {
    window.askbarAudit = { session, sessionFromStatus, loadSession, loadAccess, failureClass };

    setInterval(() => {
      if (!session.open) return;
      session.idleLeft = Math.max(0, session.idleLeft - 1);
      session.totalLeft = Math.max(0, session.totalLeft - 1);
      if (Math.min(session.idleLeft, session.totalLeft) === 0) loadSession();
      else renderSession();
    }, 1000);

    $('audit-unseal-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const passphrase = $('audit-pass').value;
      $('audit-pass').value = ''; // gone from the field the moment it is sent, whatever the answer
      show($('audit-pass-error'), false);
      try {
        applySession(await openSession(passphrase).finally(loadAccess));
      } catch (ex) {
        if (ex.message === 'Signed out') return;
        $('audit-pass-error').textContent = ex.message;
        show($('audit-pass-error'), true);
        $('audit-pass').focus();
      }
    });

    $('audit-seal').addEventListener('click', async () => {
      await closeSession();
      applySession({ open: false });
    });

    registerTab({
      id: SEALED,
      load: async () => {
        await Promise.all([loadSession(), loadAccess()]);
      },
    });
  }

  // ------------------------------------------------------------ routing

  // What kind of failure an error code stands for, in the router's own terms (see router::classify).
  function failureClass(code) {
    if (!code) return '';
    if (code === 'timeout') return 'ran out of time';
    if (code === 'kill_switch') return 'switched off';
    if (code === 'http_401' || code === 'http_403' || code === 'config') return 'bad credentials';
    const m = /^http_(\d+)$/.exec(code);
    if (m) {
      const n = Number(m[1]);
      return n === 408 || n === 429 || n >= 500 ? 'worth retrying' : 'refused outright';
    }
    if (code === 'connect' || code === 'stream') return 'worth retrying';
    return code;
  }

  // Provider names, read once; an id is shown without its name when the list cannot be read.
  let providerNames = {};
  async function loadNames() {
    try {
      const list = await api('GET', '/api/admin/v1/providers');
      providerNames = Object.fromEntries(list.map((p) => [p.id, p.name]));
    } catch (_) { /* names are a nicety; ids are shown without them */ }
  }
  const providerLabel = (id) => ui.providerLabel(providerNames, id);

  // The tries of the request in the address, in order: which provider, which try, how it ended. `missing` says why
  // there is no request to show (not in the trail), instead of the default prompt.
  function renderAttempts(trace, missing) {
    const attempts = ((trace && trace.spans) || []).filter((s) => s.kind === 'attempt').sort((a, b) => ((a.attrs && a.attrs.started_ms) || a.started_at * 1000) - ((b.attrs && b.attrs.started_ms) || b.started_at * 1000) || (a.id < b.id ? -1 : 1));
    show($('audit-attempts'), attempts.length > 0);
    const empty = $('audit-routing-empty');
    show(empty, attempts.length === 0);
    empty.textContent = missing || (trace ? 'This request has no tries recorded.' : 'Pick a request on Model calls to see how it was routed.');
    ui.rows($('audit-attempt-rows'), attempts, (a) => {
      const at = a.attrs || {};
      const tr = el('tr');
      tr.dataset.attempt = a.id;
      const cells = [
        [providerLabel(a.provider_id), 'wrap'],
        [String((at.retry_index || 0) + 1)],
        [a.status, 'status-' + a.status],
        [failureClass(a.error_code) || '–'],
        [a.error_code || '–', 'wrap'],
        [at.latency_ms != null ? ui.duration(at.latency_ms) : '–'],
      ];
      for (const [text, cls] of cells) tr.appendChild(el('td', cls, text));
      return tr;
    }, { key: (a) => a.id });
  }

  // The request in the address (?id=, from Model calls), then the providers in the chosen range: tries, retries,
  // failures, and whether each provider's credentials are currently refused.
  async function loadRouting() {
    const since = Math.floor(Date.now() / 1000) - Number($('routing-window').value) * ui.DAY;
    const id = new URLSearchParams(location.search).get('id');
    const [, stats, health] = await Promise.all([loadNames(), read('/api/admin/v1/audit/stats?since=' + since), read('/api/admin/v1/audit/provider-health')]);
    const flagged = new Map(((health && health.providers) || []).map((p) => [p.provider_id, p]));
    const rows = ((stats && stats.by_provider) || []).filter((p) => p.provider_id);
    // A provider that is flagged but had no tries in the range still deserves a row.
    for (const pid of flagged.keys()) if (!rows.some((r) => r.provider_id === pid)) rows.push({ provider_id: pid, attempts: 0, retries: 0, failed_attempts: 0 });
    show($('audit-routing-table'), rows.length > 0);
    show($('audit-routing-overall-empty'), rows.length === 0);
    ui.rows($('audit-routing-rows'), rows, (r) => {
      const tr = el('tr');
      tr.dataset.provider = r.provider_id;
      const f = flagged.get(r.provider_id);
      const cells = [
        [providerLabel(r.provider_id), 'wrap'],
        [ui.number(r.attempts || 0)],
        [ui.number(r.retries || 0)],
        [ui.number(r.failed_attempts || 0)],
        [f ? 'refused since ' + new Date(f.since * 1000).toLocaleString() + (f.error_code ? ' (' + f.error_code + ')' : '') : 'ok', f ? 'status-error wrap' : ''],
      ];
      for (const [text, cls] of cells) tr.appendChild(el('td', cls, text));
      return tr;
    }, { key: (r) => r.provider_id });
    // The request last, so its own message is the one left on screen.
    let trace;
    let missing = '';
    if (id) {
      try {
        trace = await api('GET', '/api/admin/v1/audit/traces/' + encodeURIComponent(id));
      } catch (e) {
        if (e.status === 404) missing = 'This request is not in the audit trail.';
        else if (e.message !== 'Signed out') banner(ui.explain(e));
      }
    }
    renderAttempts(trace, missing);
  }

  function startRouting() {
    window.askbarAudit = { session: { open: false }, sessionFromStatus() {}, loadSession() {}, loadAccess() {}, failureClass, loadRouting };
    $('routing-window').addEventListener('change', loadRouting);
    registerTab({ id: PAGE, load: loadRouting });
  }

  if (PAGE === SEALED) startSealed();
  else startRouting();
})();
