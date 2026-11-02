// The Audit tab: what the server recorded about each reply. Plain JavaScript, no build step.
// Everything that comes from the server (provider names, models, ids, error codes) goes in
// through textContent, never as HTML.
(() => {
  'use strict';
  const { api, registerTab, show, $ } = window.askbarAdmin;

  const SUBS = ['overview', 'requests', 'trace', 'routing', 'content'];
  const state = { sub: 'overview' };

  function setSub(name) {
    if (!SUBS.includes(name)) return;
    state.sub = name;
    for (const b of $('audit-nav').children) b.setAttribute('aria-selected', b.dataset.sub === name ? 'true' : 'false');
    for (const s of SUBS) show($('audit-' + s), s === name);
    if (name === 'routing' && window.askbarAudit) window.askbarAudit.loadRouting();
    if (name === 'content' && window.askbarAudit) window.askbarAudit.loadAccess();
  }

  // One banner for anything that stops the tab reading the audit store: the server away (502), or a
  // server with no audit store (503). The views keep their "nothing yet" text underneath.
  function banner(message) {
    const el = $('audit-error');
    el.textContent = message || '';
    show(el, !!message);
  }

  function explain(e) {
    if (e.status === 502) return 'The server is offline, so the audit store cannot be read right now. Nothing is lost: it is recorded again as soon as the server is back.';
    if (e.status === 503) return 'The server has no audit store recording.';
    return e.message;
  }

  // Runs a read and shows its failure in the banner; returns undefined on failure.
  async function read(path) {
    try {
      const data = await api('GET', path);
      banner('');
      return data;
    } catch (e) {
      if (e.message !== 'Signed out') banner(explain(e));
      return undefined;
    }
  }

  // ------------------------------------------------------------ formatting

  const number = (n) => Number(n).toLocaleString('en-US');
  function duration(ms) {
    if (ms == null) return '–';
    return ms >= 1000 ? (Math.round(ms / 100) / 10) + ' s' : Math.round(ms) + ' ms';
  }
  // A cost is a number or null; null is shown as "unknown", never as 0.
  function money(c) {
    if (c == null) return 'unknown';
    return c === 0 ? '0' : String(Math.round(c * 1e6) / 1e6);
  }

  function el(tag, cls, text) {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined) n.textContent = text;
    return n;
  }

  // ------------------------------------------------------------- overview

  function tile(stat, value, label) {
    const t = el('div', 'tile');
    const v = el('div', 'v', value);
    v.dataset.stat = stat;
    t.append(v, el('div', 'l', label));
    return t;
  }

  // A list of rows with a bar sized by `requests`, so the busy days and providers stand out.
  function barList(box, rows) {
    box.textContent = '';
    const max = Math.max(1, ...rows.map((r) => r.requests));
    for (const r of rows) {
      const row = el('div', 'bar-row');
      row.append(el('span', 'name', r.label));
      const track = el('span', 'track');
      const fill = el('span', 'fill');
      fill.style.width = Math.round((100 * r.requests) / max) + '%';
      track.appendChild(fill);
      row.append(track, el('span', 'figs', number(r.requests) + ' req · ' + number(r.tokens) + ' tok · ' + money(r.cost)));
      box.appendChild(row);
    }
  }

  let providerNames = {};

  function renderOverview(s) {
    const empty = !s || !s.requests;
    show($('audit-overview-empty'), empty);
    show($('audit-tiles'), !empty);
    show($('audit-days-title'), !empty && (s.by_day || []).length > 0);
    show($('audit-providers-title'), !empty && (s.by_provider || []).length > 0);
    const note = $('audit-cost-note');
    if (empty) {
      $('audit-tiles').textContent = '';
      $('audit-days').textContent = '';
      $('audit-by-provider').textContent = '';
      show(note, false);
      return;
    }
    const t = $('audit-tiles');
    t.textContent = '';
    const pct = s.error_rate == null ? '–' : (Math.round(s.error_rate * 1000) / 10) + '%';
    t.append(
      tile('requests', number(s.requests), 'Requests'),
      tile('error-rate', pct, 'Error rate'),
      tile('latency-p50', duration(s.latency_ms && s.latency_ms.p50), 'Latency, median'),
      tile('latency-p95', duration(s.latency_ms && s.latency_ms.p95), 'Latency, 95th percentile'),
      tile('ttft-p50', duration(s.ttft_ms && s.ttft_ms.p50), 'First token, median'),
      tile('ttft-p95', duration(s.ttft_ms && s.ttft_ms.p95), 'First token, 95th percentile'),
      tile('tokens-in', number((s.tokens || {}).in || 0), 'Tokens in'),
      tile('tokens-out', number((s.tokens || {}).out || 0), 'Tokens out'),
      tile('tokens-cached', number((s.tokens || {}).cached || 0), 'Tokens cached'),
      tile('cost', money(s.cost), 'Cost'),
    );
    show(note, s.cost == null && s.cost_unknown_requests > 0);
    note.textContent = s.cost == null && s.cost_unknown_requests > 0
      ? 'Cost is unknown for ' + number(s.cost_unknown_requests) + ' request(s) because a price is missing for their provider. Set prices on the Providers tab.'
      : '';
    barList($('audit-days'), (s.by_day || []).map((d) => ({ label: d.day, requests: d.requests, tokens: (d.tokens.in || 0) + (d.tokens.out || 0), cost: d.cost })));
    barList($('audit-by-provider'), (s.by_provider || []).map((p) => ({ label: providerNames[p.provider_id] || p.provider_id || '(none)', requests: p.requests, tokens: (p.tokens.in || 0) + (p.tokens.out || 0), cost: p.cost })));
  }

  async function loadOverview() {
    const since = Math.floor(Date.now() / 1000) - Number($('audit-window').value);
    try {
      const list = await api('GET', '/admin/api/providers');
      providerNames = Object.fromEntries(list.map((p) => [p.id, p.name]));
    } catch (_) { /* names are a nicety; ids are shown without them */ }
    const stats = await read('/admin/api/audit/stats?since=' + since);
    if (stats) renderOverview(stats);
  }

  $('audit-window').addEventListener('change', loadOverview);

  // ------------------------------------------------------------- requests

  const PAGE = 25;
  const requests = { status: '', provider: '', last: null };

  // The query for one page of traces: the filters, a cursor (the id of the last row already shown)
  // and the page size. Empty filters are left out.
  function requestsQuery({ status, provider, before }) {
    const q = new URLSearchParams();
    if (status) q.set('status', status);
    if (provider) q.set('provider', provider);
    if (before) q.set('before', before);
    q.set('limit', String(PAGE));
    return q.toString();
  }

  function requestRow(t) {
    const tr = el('tr');
    tr.dataset.trace = t.trace_id;
    tr.tabIndex = 0;
    tr.setAttribute('role', 'button');
    const when = t.started_at ? new Date(t.started_at * 1000).toLocaleString() : '–';
    const cells = [
      [when],
      [t.user_id || '–', 'wrap'],
      [t.provider_id ? (providerNames[t.provider_id] || t.provider_id) : '–', 'wrap'],
      [t.model || '–', 'wrap'],
      [t.status, 'status-' + t.status],
      [duration(t.latency_ms)],
      [t.tokens_in == null && t.tokens_out == null ? '–' : number(t.tokens_in || 0) + ' → ' + number(t.tokens_out || 0)],
      [t.tokens_in == null && t.tokens_out == null ? '–' : money(t.cost)],
    ];
    for (const [text, cls] of cells) tr.appendChild(el('td', cls, text));
    return tr;
  }

  async function loadRequests(more) {
    if (!more) {
      $('audit-rows').textContent = '';
      requests.last = null;
    }
    const list = await read('/admin/api/audit/traces?' + requestsQuery({ status: requests.status, provider: requests.provider, before: more ? requests.last : null }));
    if (!list) return;
    const rows = list.traces || [];
    for (const t of rows) $('audit-rows').appendChild(requestRow(t));
    if (rows.length) requests.last = rows[rows.length - 1].trace_id;
    const any = $('audit-rows').children.length > 0;
    show($('audit-table'), any);
    show($('audit-requests-empty'), !any);
    $('audit-requests-empty').textContent = requests.status || requests.provider ? 'No requests match these filters.' : 'No requests recorded yet.';
    show($('audit-more'), rows.length >= PAGE);
  }

  function fillProviderFilter() {
    const sel = $('audit-f-provider');
    const keep = sel.value;
    sel.textContent = '';
    sel.appendChild(Object.assign(el('option', '', 'Any'), { value: '' }));
    for (const [id, name] of Object.entries(providerNames)) sel.appendChild(Object.assign(el('option', '', name), { value: id }));
    sel.value = keep;
  }

  $('audit-f-status').addEventListener('change', () => { requests.status = $('audit-f-status').value; loadRequests(false); });
  $('audit-f-provider').addEventListener('change', () => { requests.provider = $('audit-f-provider').value; loadRequests(false); });
  $('audit-more').addEventListener('click', () => loadRequests(true));
  function openFromRow(e) {
    const tr = e.target.closest('tr[data-trace]');
    if (tr) openTrace(tr.dataset.trace);
  }
  $('audit-rows').addEventListener('click', openFromRow);
  $('audit-rows').addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openFromRow(e); } });

  // ---------------------------------------------------------------- trace

  // Where each span goes in the waterfall, as percentages of the trace's whole duration.
  // `spans` are as the server sends them; times come from attrs.started_ms and attrs.ended_ms
  // (milliseconds) and fall back to the whole-second columns. A span with no end is still running:
  // it is drawn up to `nowMs`. Rows are in the order the spans began, each indented by how many
  // parents it has. Pure, so it can be tested with fixed numbers.
  function layout(spans, nowMs) {
    const t0 = (s) => (s.attrs && s.attrs.started_ms) || s.started_at * 1000;
    const t1 = (s) => (s.attrs && s.attrs.ended_ms) || (s.ended_at ? s.ended_at * 1000 : null);
    const rows = spans.map((s) => ({ span: s, start: t0(s), end: t1(s) }));
    for (const r of rows) r.running = r.end == null;
    for (const r of rows) if (r.running) r.end = Math.max(r.start, nowMs);
    rows.sort((a, b) => a.start - b.start || (a.span.id < b.span.id ? -1 : 1));
    const byId = new Map(rows.map((r) => [r.span.id, r]));
    const depth = (r, seen = new Set()) => {
      const p = r.span.parent_id && byId.get(r.span.parent_id);
      if (!p || seen.has(r.span.id)) return 0;
      seen.add(r.span.id);
      return 1 + depth(p, seen);
    };
    const start = rows.length ? Math.min(...rows.map((r) => r.start)) : 0;
    const end = rows.length ? Math.max(...rows.map((r) => r.end)) : 0;
    const total = Math.max(1, end - start);
    const pct = (n) => Math.round(n * 100) / 100;
    return {
      start, end, total,
      rows: rows.map((r) => ({
        span: r.span,
        depth: depth(r),
        left: pct((100 * (r.start - start)) / total),
        width: pct(Math.max(0.5, (100 * (r.end - r.start)) / total)),
        durationMs: r.end - r.start,
        offsetMs: r.start - start,
        running: r.running,
      })),
    };
  }

  function spanDetails(row) {
    const s = row.span;
    const lines = [
      s.kind + ' · ' + s.name + ' · ' + s.status + (row.running ? ' (still running)' : ''),
      'starts at +' + row.offsetMs + ' ms, takes ' + row.durationMs + ' ms',
    ];
    if (s.provider_id) lines.push('provider ' + (providerNames[s.provider_id] || s.provider_id) + (s.model ? ' · ' + s.model : ''));
    if (s.tokens_in != null || s.tokens_out != null) lines.push('tokens ' + number(s.tokens_in || 0) + ' in, ' + number(s.tokens_out || 0) + ' out' + (s.tokens_cached ? ', ' + number(s.tokens_cached) + ' cached' : ''));
    if (s.ttft_ms != null) lines.push('first token after ' + s.ttft_ms + ' ms');
    if (s.cost != null) lines.push('cost ' + money(s.cost));
    if (s.error_code) lines.push('error ' + s.error_code);
    const extra = Object.fromEntries(Object.entries(s.attrs || {}).filter(([k]) => k !== 'started_ms' && k !== 'ended_ms'));
    if (Object.keys(extra).length) lines.push(JSON.stringify(extra));
    return lines.join('\n');
  }

  function renderTrace(trace) {
    const box = $('audit-waterfall');
    box.textContent = '';
    const lay = layout(trace.spans || [], Date.now());
    show($('audit-trace-empty'), false);
    show(box, true);
    const head = $('audit-trace-head');
    head.textContent = '';
    const turn = lay.rows.find((r) => r.span.kind === 'turn');
    head.append(
      el('p', '', 'Request ' + trace.trace_id + (turn ? ' · ' + turn.span.status : '') + ' · ' + duration(lay.total) + ' · cost ' + money(trace.cost)),
    );
    show(head, true);
    const details = $('audit-span-details');
    for (const row of lay.rows) {
      const line = el('div', 'span-row');
      line.setAttribute('role', 'listitem');
      const label = el('span', 'label');
      label.style.paddingLeft = row.depth * 14 + 'px';
      label.append(el('span', 'kind', row.span.kind + ' '), document.createTextNode(row.span.name + ' · ' + row.span.status));
      const track = el('span', 'track');
      const bar = el('span', 'span-bar status-' + row.span.status + (row.running ? ' running' : ''));
      bar.dataset.span = row.span.id;
      bar.tabIndex = 0;
      bar.style.left = row.left + '%';
      bar.style.width = row.width + '%';
      bar.setAttribute('aria-label', row.span.kind + ' ' + row.span.name + ', ' + row.span.status + ', ' + duration(row.durationMs));
      const show_ = () => { details.textContent = spanDetails(row); show(details, true); };
      bar.addEventListener('mouseenter', show_);
      bar.addEventListener('focus', show_);
      track.appendChild(bar);
      line.append(label, track);
      box.appendChild(line);
    }
    show(details, false);
  }

  // ---------------------------------------------------------------- routing

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

  const providerLabel = (id) => (id ? providerNames[id] || id : '–');

  // The tries of the open request, in order: which provider, which try, whom it stood in for, how it ended.
  function renderAttempts(trace) {
    const attempts = ((trace && trace.spans) || []).filter((s) => s.kind === 'attempt').sort((a, b) => ((a.attrs && a.attrs.started_ms) || a.started_at * 1000) - ((b.attrs && b.attrs.started_ms) || b.started_at * 1000) || (a.id < b.id ? -1 : 1));
    const body = $('audit-attempt-rows');
    body.textContent = '';
    show($('audit-attempts'), attempts.length > 0);
    show($('audit-routing-empty'), attempts.length === 0);
    for (const a of attempts) {
      const at = a.attrs || {};
      const tr = el('tr');
      tr.dataset.attempt = a.id;
      const cells = [
        [providerLabel(a.provider_id), 'wrap'],
        [String((at.retry_index || 0) + 1)],
        [at.fallback_from ? providerLabel(at.fallback_from) : '–', 'wrap'],
        [a.status, 'status-' + a.status],
        [failureClass(a.error_code) || '–'],
        [a.error_code || '–', 'wrap'],
        [at.latency_ms != null ? duration(at.latency_ms) : '–'],
      ];
      for (const [text, cls] of cells) tr.appendChild(el('td', cls, text));
      body.appendChild(tr);
    }
  }

  // Per provider in the window: tries, retries, fallbacks served, failures, and whether its
  // credentials are currently refused.
  async function loadRouting() {
    renderAttempts(state.trace);
    const since = Math.floor(Date.now() / 1000) - Number($('audit-window').value);
    const [stats, health] = await Promise.all([read('/admin/api/audit/stats?since=' + since), read('/admin/api/audit/provider-health')]);
    const flagged = new Map(((health && health.providers) || []).map((p) => [p.provider_id, p]));
    const rows = ((stats && stats.by_provider) || []).filter((p) => p.provider_id);
    // A provider that is flagged but had no tries in the window still deserves a row.
    for (const id of flagged.keys()) if (!rows.some((r) => r.provider_id === id)) rows.push({ provider_id: id, attempts: 0, retries: 0, fallbacks: 0, failed_attempts: 0 });
    const body = $('audit-routing-rows');
    body.textContent = '';
    show($('audit-routing-table'), rows.length > 0);
    show($('audit-routing-overall-empty'), rows.length === 0);
    for (const r of rows) {
      const tr = el('tr');
      tr.dataset.provider = r.provider_id;
      const f = flagged.get(r.provider_id);
      const cells = [
        [providerLabel(r.provider_id), 'wrap'],
        [number(r.attempts || 0)],
        [number(r.retries || 0)],
        [number(r.fallbacks || 0)],
        [number(r.failed_attempts || 0)],
        [f ? 'refused since ' + new Date(f.since * 1000).toLocaleString() + (f.error_code ? ' (' + f.error_code + ')' : '') : 'ok', f ? 'status-error wrap' : ''],
      ];
      for (const [text, cls] of cells) tr.appendChild(el('td', cls, text));
      body.appendChild(tr);
    }
  }

  // ---------------------------------------------------------------- content

  // The spans of the open request that have sealed content, each with a View button. Nothing is
  // fetched until View is clicked and a passphrase is typed.
  function renderSealed(trace) {
    const box = $('audit-sealed');
    box.textContent = '';
    const sealed = ((trace && trace.spans) || []).filter((s) => s.sealed > 0);
    show($('audit-content-empty'), sealed.length === 0);
    $('audit-content-empty').textContent = trace ? 'This request has no sealed content.' : 'Pick a request on the Requests view to see its sealed content.';
    for (const s of sealed) {
      const row = el('div', 'sealed-row');
      row.dataset.span = s.id;
      row.append(el('span', 'what', s.kind + ' · ' + s.name + ' · Sealed (' + s.sealed + (s.sealed === 1 ? ' item)' : ' items)')));
      const view = el('button', 'secondary view-btn', 'View');
      view.type = 'button';
      view.dataset.span = s.id;
      view.addEventListener('click', () => askPassphrase(s));
      row.appendChild(view);
      box.appendChild(row);
    }
  }

  // Every attempt to open sealed content, newest first, from the admin's own record.
  async function loadAccess() {
    const r = await read('/admin/api/audit/access');
    if (!r) return;
    const body = $('audit-access-rows');
    body.textContent = '';
    show($('audit-access'), r.access.length > 0);
    show($('audit-access-empty'), r.access.length === 0);
    for (const a of r.access) {
      const tr = el('tr');
      tr.dataset.access = String(a.id);
      for (const [text, cls] of [
        [new Date(a.opened_at * 1000).toLocaleString()],
        [a.span_id, 'wrap'],
        [a.key_id],
        [a.ok ? 'opened' : 'wrong passphrase', a.ok ? '' : 'status-error'],
      ]) tr.appendChild(el('td', cls, text));
      body.appendChild(tr);
    }
  }

  const dialog = $('audit-pass-dialog');
  let plainTimer = null;

  // Wipes whatever was opened, and the passphrase field, from the page.
  function wipe() {
    clearTimeout(plainTimer);
    plainTimer = null;
    $('audit-plain').textContent = '';
    $('audit-pass').value = '';
  }

  function closeDialog() {
    wipe();
    if (dialog.open) dialog.close();
  }

  function askPassphrase(span) {
    wipe();
    state.opening = span;
    $('audit-pass-what').textContent = span.kind + ' · ' + span.name + ' (' + span.sealed + (span.sealed === 1 ? ' sealed item' : ' sealed items') + ')';
    show($('audit-pass-error'), false);
    show($('audit-ask'), true);
    $('audit-pass-ok').hidden = false;
    $('audit-pass-cancel').textContent = 'Cancel';
    if (!dialog.open) dialog.showModal();
    $('audit-pass').focus();
  }

  $('audit-pass-cancel').addEventListener('click', closeDialog);
  dialog.addEventListener('close', wipe);

  $('audit-pass-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    if (!state.opening) return;
    const passphrase = $('audit-pass').value;
    $('audit-pass').value = ''; // gone from the field the moment it is sent, whatever the answer
    show($('audit-pass-error'), false);
    try {
      const r = await api('POST', '/admin/api/audit/payloads/' + encodeURIComponent(state.opening.id) + '/open', { passphrase }).finally(loadAccess);
      const plain = $('audit-plain');
      plain.textContent = '';
      for (const p of r.payloads) {
        plain.appendChild(el('strong', '', p.role + ' (key ' + p.key_id + ')'));
        plain.appendChild(document.createTextNode('\n' + (p.text == null ? '(could not be opened)' : p.text) + '\n\n'));
      }
      if (!r.payloads.length) plain.textContent = 'Nothing is sealed on this span.';
      show($('audit-ask'), false);
      $('audit-pass-ok').hidden = true;
      $('audit-pass-cancel').textContent = 'Close';
      // What was opened does not stay on the page: a minute at most.
      clearTimeout(plainTimer);
      plainTimer = setTimeout(closeDialog, state.plainMs || 60000);
    } catch (ex) {
      if (ex.message === 'Signed out') { closeDialog(); return; }
      $('audit-pass-error').textContent = ex.message;
      show($('audit-pass-error'), true);
      $('audit-pass').focus();
    }
  });

  // Opens one request on the Trace view.
  async function openTrace(id) {
    state.traceId = id;
    setSub('trace');
    const trace = await read('/admin/api/audit/traces/' + encodeURIComponent(id));
    if (trace && state.traceId === id) {
      state.trace = trace;
      renderTrace(trace);
      renderAttempts(trace);
      renderSealed(trace);
    }
  }

  async function load() {
    setSub(state.sub);
    await loadOverview();
    fillProviderFilter();
    await loadRequests(false);
  }

  $('audit-nav').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-sub]');
    if (b) setSub(b.dataset.sub);
  });

  window.askbarAudit = { setSub, read, banner, state, renderOverview, requestsQuery, openTrace, layout, renderTrace, loadRouting, failureClass, renderSealed, loadAccess };
  registerTab({ id: 'audit', label: 'Audit', load });
})();
