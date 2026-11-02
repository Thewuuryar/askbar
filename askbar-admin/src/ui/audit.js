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

  async function load() {
    setSub(state.sub);
    await loadOverview();
  }

  $('audit-nav').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-sub]');
    if (b) setSub(b.dataset.sub);
  });

  window.askbarAudit = { setSub, read, banner, state, renderOverview };
  registerTab({ id: 'audit', label: 'Audit', load });
})();
