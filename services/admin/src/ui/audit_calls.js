// Audit / Model calls (plan step 6.1): the figures for a range, the list of calls, and the selected call's trace in the
// docked side panel. Plain JavaScript, no build step. Everything that comes from the server (provider names, models, ids,
// error codes) goes in through textContent, never as HTML. Sealed content is opened only through the admin's own
// session (app.js openSession) and shown only inside the panel.
(() => {
  'use strict';
  const { api, registerTab, show, $ } = window.askbarAdmin;
  const { ui } = window;

  const PANEL = 'trace-panel';
  const size = () => ui.constants.AUDIT_PAGE_SIZE;

  const state = {
    days: 1, status: '', provider: '',
    first: [], extra: [], lastPageDone: true,
    names: {}, namesRead: false,
    selected: null, seq: 0, trace: null, pending: null,
  };
  const session = { open: false };

  // ------------------------------------------------------------ formatting

  // A cost is a number or null; null is shown as "unknown", never as 0.
  function money(c) {
    if (c == null) return 'unknown';
    return c === 0 ? '0' : String(Math.round(c * 1e6) / 1e6);
  }
  const whenText = (secs) => (secs ? new Date(secs * ui.MS).toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : '–');
  const providerLabel = (id) => ui.providerLabel(state.names, id);
  const since = () => Math.floor(Date.now() / ui.MS) - state.days * ui.DAY;

  // The state and the word for a status, so it is never colour alone (rule 37).
  const STATUS = {
    done: ['ok', 'done'], error: ['error', 'error'], timed_out: ['error', 'timed out'],
    cancelled: ['off', 'cancelled'], interrupted: ['warning', 'interrupted'], running: ['pending', 'running'],
  };
  function statusNode(status) {
    const [kind, word] = STATUS[status] || ['off', String(status || 'unknown').replace(/_/g, ' ')];
    return ui.status(kind, word);
  }

  // The address keeps the filters (and the open trace, which ui.openPanel owns): replace, never push, and keep the
  // panel's own history entry.
  function setAddress(change) {
    const u = new URL(location.href);
    change(u.searchParams);
    history.replaceState(history.state, '', u.pathname + u.search + u.hash);
  }

  // ------------------------------------------------------------ errors

  const banner = (message) => ui.inlineError('calls-error', message);

  // ------------------------------------------------------------ the figures

  function renderKpis(s) {
    const put = (id, node) => $(id).replaceChildren(node);
    const dash = () => ui.el('span', { class: 'num', text: '–' });
    const ms = (v) => (v == null ? dash() : ui.el('span', { class: 'num', text: ui.duration(v) }));
    if (!s) { for (const id of ['kpi-replies', 'kpi-errors', 'kpi-tokens', 'kpi-p50', 'kpi-p95']) put(id, dash()); return; }
    // Errors are what the auditor counts in its error rate: `error` and `timed_out`.
    const by = s.by_status;
    const errors = by ? (by.error || 0) + (by.timed_out || 0) : null;
    put('kpi-replies', s.requests == null ? dash() : ui.num(s.requests));
    put('kpi-errors', errors == null ? dash() : ui.num(errors));
    $('kpi-errors').classList.toggle('bad', errors > 0);
    put('kpi-tokens', s.tokens ? ui.num((s.tokens.in || 0) + (s.tokens.out || 0)) : dash());
    put('kpi-p50', ms(s.latency_ms && s.latency_ms.p50));
    put('kpi-p95', ms(s.latency_ms && s.latency_ms.p95));
  }

  // ------------------------------------------------------------ the list

  function callRow(t) {
    const when = whenText(t.started_at);
    const open = ui.el('button', {
      type: 'button', class: 'btn btn-quiet row-link', 'data-open': t.trace_id, text: when,
      'aria-label': 'Open the trace of the call at ' + when + (t.user_id ? ' for ' + t.user_id : ''),
    });
    const tokens = t.tokens_in == null && t.tokens_out == null ? '–' : ui.number(t.tokens_in || 0) + ' / ' + ui.number(t.tokens_out || 0);
    const clip = (cls, text) => ui.el('td', { class: 'clip ' + cls, title: text === '–' ? null : text, text });
    const model = t.model || '–';
    const tr = ui.el('tr', { 'data-trace': t.trace_id, class: t.status === 'error' || t.status === 'timed_out' ? 'call-err' : null },
      ui.el('td', null, open),
      clip('col-user', t.user_id || '–'),
      clip('col-provider', providerLabel(t.provider_id)),
      clip('', model),
      ui.el('td', { class: 'num col-tokens', text: tokens }),
      ui.el('td', { class: 'num col-duration', text: ui.duration(t.latency_ms) }),
      ui.el('td', null, statusNode(t.status)));
    if (t.trace_id === state.selected) mark(tr, true);
    return tr;
  }
  function mark(tr, on) {
    tr.classList.toggle('call-sel', on);
    const b = tr.querySelector('.row-link');
    if (b) { if (on) b.setAttribute('aria-current', 'true'); else b.removeAttribute('aria-current'); }
  }
  // The selected row follows the open trace without being rebuilt, so focus and scroll stay where they are.
  function markSelected() {
    for (const tr of $('calls-rows').querySelectorAll('tr[data-trace]')) mark(tr, tr.dataset.trace === state.selected);
  }

  const pageDone = (page) => page.length < size() || page.some((t) => t.started_at != null && t.started_at < since());
  // The range filters the list on this side: a row older than the range is not shown, and nothing beyond it is asked for.
  const shown = () => {
    const have = new Set(state.first.map((t) => t.trace_id));
    return [...state.first, ...state.extra.filter((t) => !have.has(t.trace_id))].filter((t) => t.started_at == null || t.started_at >= since());
  };

  function fillProviders() {
    const sel = $('calls-provider');
    sel.replaceChildren(ui.el('option', { value: '', text: 'Any' }));
    const ids = Object.keys(state.names);
    // A provider named in the address that is gone still filters: it is listed by its id.
    if (state.provider && !ids.includes(state.provider)) ids.push(state.provider);
    for (const id of ids) sel.append(ui.el('option', { value: id, text: providerLabel(id) }));
    sel.value = state.provider;
  }

  function draw(quiet) {
    const rows = shown();
    ui.rows($('calls-rows'), rows, callRow, { key: (t) => t.trace_id, fresh: !quiet, noun: ['call', 'calls'] });
    show($('calls-table'), rows.length > 0);
    const empty = $('calls-empty');
    show(empty, rows.length === 0);
    if (rows.length === 0) empty.replaceChildren(ui.empty(state.status || state.provider ? 'No model calls match these filters.' : 'No model calls recorded in this range.'));
    $('calls-count').textContent = rows.length === 0 ? '' : 'Showing ' + ui.number(rows.length) + (rows.length === 1 ? ' call' : ' calls');
    show($('calls-more'), rows.length > 0 && !state.lastPageDone);
  }

  // One read of everything the list needs, every LIVE_EVERY_SECS while the tab is visible (rule 34). It reads the first
  // page; pages added with Load more are kept and merged under it.
  async function readAll() {
    const q = new URLSearchParams();
    if (state.status) q.set('status', state.status);
    if (state.provider) q.set('provider', state.provider);
    q.set('limit', String(size()));
    const namesRead = state.namesRead ? null : api('GET', '/api/admin/v1/providers').catch(() => null);
    const [stats, list, providers] = await Promise.all([
      api('GET', '/api/admin/v1/audit/stats?since=' + since()),
      api('GET', '/api/admin/v1/audit/traces?' + q.toString()),
      namesRead,
    ]);
    delete stats.since; // it changes with every read and means nothing on screen
    return { stats, traces: (list && list.traces) || [], providers };
  }

  let quietNext = false;
  const live = ui.live(readAll, {
    when: () => !$('app').hidden,
    notice: $('calls-kpis'),
    render(data, ctx) {
      if (data.providers) {
        state.names = Object.fromEntries(data.providers.map((p) => [p.id, p.name]));
        state.namesRead = true;
        fillProviders();
        if (state.trace) renderMeta(state.trace); // a call opened from the address may have drawn before the names arrived
      }
      banner('');
      renderKpis(data.stats);
      state.first = data.traces;
      if (state.extra.length === 0) state.lastPageDone = pageDone(data.traces);
      draw(ctx.first || ctx.force || quietNext);
      quietNext = false;
    },
    onError(e) { if (e.message !== 'Signed out') banner(ui.explain(e)); },
  });

  // A change of filter or range starts again from the newest call.
  function restart() {
    state.extra = [];
    quietNext = true;
    return live.refresh({ force: true });
  }

  $('calls-window').addEventListener('change', () => { state.days = Number($('calls-window').value); restart(); });
  $('calls-status').addEventListener('change', () => {
    state.status = $('calls-status').value;
    setAddress((p) => { if (state.status) p.set('status', state.status); else p.delete('status'); });
    restart();
  });
  $('calls-provider').addEventListener('change', () => {
    state.provider = $('calls-provider').value;
    setAddress((p) => { if (state.provider) p.set('provider', state.provider); else p.delete('provider'); });
    restart();
  });

  $('calls-more').addEventListener('click', async () => {
    const rows = shown();
    const last = rows[rows.length - 1];
    if (!last) return;
    const done = ui.busy($('calls-more'), 'Loading…');
    try {
      const q = new URLSearchParams();
      if (state.status) q.set('status', state.status);
      if (state.provider) q.set('provider', state.provider);
      q.set('before', last.trace_id);
      q.set('limit', String(size()));
      const list = await api('GET', '/api/admin/v1/audit/traces?' + q.toString());
      const page = (list && list.traces) || [];
      state.extra.push(...page);
      state.lastPageDone = pageDone(page);
      banner('');
      draw(true); // older calls, not new ones: no highlight
    } catch (e) {
      if (e.message !== 'Signed out') banner(ui.explain(e));
    } finally {
      done();
    }
  });

  $('calls-rows').addEventListener('click', (e) => {
    const tr = e.target.closest('tr[data-trace]');
    if (tr) openTrace(tr.dataset.trace);
  });

  // ------------------------------------------------------------ the trace panel

  // Where each span goes in the waterfall, as percentages of the trace's whole duration. Times come from attrs.started_ms
  // and attrs.ended_ms and fall back to the whole-second columns. A span with no end is still running: it is drawn up to
  // `nowMs`. Rows are in the order the spans began, indented by how many parents each has. Pure, so it can be tested.
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
    if (s.provider_id) lines.push('provider ' + providerLabel(s.provider_id) + (s.model ? ' · ' + s.model : ''));
    if (s.tokens_in != null || s.tokens_out != null) lines.push('tokens ' + ui.number(s.tokens_in || 0) + ' in, ' + ui.number(s.tokens_out || 0) + ' out' + (s.tokens_cached ? ', ' + ui.number(s.tokens_cached) + ' cached' : ''));
    if (s.ttft_ms != null) lines.push('first token after ' + s.ttft_ms + ' ms');
    if (s.cost != null) lines.push('cost ' + money(s.cost));
    if (s.error_code) lines.push('error ' + s.error_code);
    const extra = Object.fromEntries(Object.entries(s.attrs || {}).filter(([k]) => k !== 'started_ms' && k !== 'ended_ms'));
    if (Object.keys(extra).length) lines.push(JSON.stringify(extra));
    return lines.join('\n');
  }

  // How a span's bar is coloured: by kind (the gate is grey, the turn green, each provider attempt blue). A failed span is red
  // whatever its kind, and its row says so in words (rule 37).
  const KINDS = ['gate', 'turn', 'attempt'];
  const FAILED = ['error', 'timed_out', 'blocked'];

  // An attempt after the first is a retry when it went to the same provider as the one before, a fallback when it did not.
  function attemptNotes(rows) {
    const notes = new Map();
    let prev = null;
    for (const r of rows.filter((x) => x.span.kind === 'attempt')) {
      if (prev) notes.set(r.span.id, prev.span.provider_id === r.span.provider_id ? 'retry' : 'fallback');
      prev = r;
    }
    return notes;
  }

  // The time scale under the bars: the start, the middle and the end of the call, in the same units as the durations.
  function renderAxis(total) {
    const axis = $('trace-axis');
    axis.replaceChildren();
    if (total == null) return;
    axis.append(ui.el('span'), ui.el('span', { class: 'axis-track' },
      ui.el('span', { class: 'tick tick-start', 'data-tick': 'start', text: ui.duration(0) }),
      ui.el('span', { class: 'tick tick-mid', 'data-tick': 'mid', text: ui.duration(total / 2) }),
      ui.el('span', { class: 'tick tick-end', 'data-tick': 'end', text: ui.duration(total) })), ui.el('span'));
  }

  function renderWaterfall(trace) {
    const box = $('trace-waterfall');
    const details = $('trace-span-details');
    box.replaceChildren();
    const lay = layout(trace.spans || [], Date.now());
    const notes = attemptNotes(lay.rows);
    const legend = new Set();
    for (const row of lay.rows) {
      const s = row.span;
      const label = ui.el('span', { class: 'label' }, ui.el('span', { class: 'kind', text: s.kind + ' ' }), s.name);
      if (notes.has(s.id)) label.append(' ', ui.el('span', { class: 'kind', text: '(' + notes.get(s.id) + ')' }));
      if (s.status !== 'done') label.append(' ', statusNode(s.status));
      label.style.paddingLeft = row.depth * 14 + 'px';
      const kind = KINDS.includes(s.kind) ? ' kind-' + s.kind : '';
      const bar = ui.el('span', {
        class: 'span-bar' + kind + ' status-' + s.status + (row.running ? ' running' : ''), 'data-span': s.id, tabindex: '0', role: 'img',
        'aria-label': s.kind + ' ' + s.name + ', ' + s.status + ', ' + ui.duration(row.durationMs) + (notes.has(s.id) ? ', ' + notes.get(s.id) : ''),
      });
      bar.style.left = row.left + '%';
      bar.style.width = row.width + '%';
      const showDetails = () => { details.textContent = spanDetails(row); show(details, true); };
      bar.addEventListener('mouseenter', showDetails);
      bar.addEventListener('focus', showDetails);
      box.append(ui.el('div', { class: 'span-row', role: 'listitem' }, label, ui.el('span', { class: 'track' }, bar), ui.el('span', { class: 'dur num', text: ui.duration(row.durationMs) })));
      legend.add(FAILED.includes(s.status) ? 'failed' : KINDS.includes(s.kind) ? s.kind : 'other');
    }
    renderAxis(lay.rows.length ? lay.total : null);
    renderLegend(legend);
    show(details, false);
    $('trace-total').textContent = '· ' + ui.duration(lay.total) + ' total';
  }

  // What each colour means, for the kinds drawn. The words are in the rows too, so this is a key, not the only carrier.
  const LEGEND = { gate: 'gate', turn: 'turn', attempt: 'provider attempt', failed: 'failed', other: 'other' };
  function renderLegend(used) {
    const box = $('trace-legend');
    box.replaceChildren(...Object.keys(LEGEND).filter((k) => used.has(k)).map((k) => ui.el('li', null, ui.el('span', { class: 'swatch swatch-' + k, 'aria-hidden': 'true' }), LEGEND[k])));
    show(box, used.size > 0);
  }

  // The facts of one call, read from its spans: the turn (who, when, how many tokens) and the last try (which provider and
  // model answered).
  function renderMeta(trace) {
    const spans = trace.spans || [];
    const turn = spans.find((s) => s.kind === 'turn') || null;
    const tries = spans.filter((s) => s.kind === 'attempt').sort((a, b) => ((a.attrs && a.attrs.started_ms) || a.started_at * 1000) - ((b.attrs && b.attrs.started_ms) || b.started_at * 1000));
    const last = tries[tries.length - 1] || null;
    const tokens = turn && (turn.tokens_in != null || turn.tokens_out != null)
      ? ui.number(turn.tokens_in || 0) + ' in / ' + ui.number(turn.tokens_out || 0) + ' out' + (turn.tokens_cached ? ' / ' + ui.number(turn.tokens_cached) + ' cached' : '')
      : '–';
    const ttft = (last && last.ttft_ms != null ? last.ttft_ms : turn && turn.ttft_ms != null ? turn.ttft_ms : null);
    const facts = [
      ['Time', turn ? whenText(turn.started_at) : '–'],
      ['User', (turn && turn.user_id) || '–'],
      ['Provider', providerLabel(last && last.provider_id)],
      ['Model', (last && last.model) || (turn && turn.model) || '–'],
      ['Tokens', tokens],
      ['First token', ttft == null ? '–' : ui.duration(ttft)],
      ['Tries', tries.length ? String(tries.length) : '–'],
      ['Cost', money(trace.cost)],
    ];
    $('trace-meta').replaceChildren(...facts.flatMap(([k, v]) => [ui.el('dt', { text: k }), ui.el('dd', { text: v })]));
    $('trace-state').replaceChildren(statusNode(turn ? turn.status : 'unknown'));
    $('trace-id').textContent = trace.trace_id;
  }

  function sealedNote() {
    const note = $('trace-sealed-note');
    const spans = ((state.trace && state.trace.spans) || []).filter((s) => s.sealed > 0);
    if (!state.trace) note.textContent = '';
    else if (spans.length === 0) note.textContent = 'Nothing is sealed on this call.';
    else if (session.open) note.textContent = 'Unsealed. Each item you view is recorded.';
    else note.textContent = 'Sealed. Opening needs the audit passphrase, and is recorded.';
  }

  // The spans of the open call that hold sealed content, each with a View button. Nothing is fetched until View is chosen
  // and a session is open.
  function renderSealed(trace) {
    const box = $('trace-sealed');
    box.replaceChildren();
    for (const s of (trace.spans || []).filter((x) => x.sealed > 0)) {
      const label = s.kind + ' · ' + s.name;
      box.append(ui.el('div', { class: 'sealed-row', 'data-span': s.id },
        ui.el('span', { class: 'what', text: label + ' · Sealed (' + s.sealed + (s.sealed === 1 ? ' item)' : ' items)') }),
        ui.el('button', { type: 'button', class: 'btn btn-secondary btn-s view-btn', 'data-span': s.id, text: 'View', 'aria-label': 'View the sealed content of ' + label, onclick: () => viewSpan(s) })));
    }
    sealedNote();
  }

  const wipe = () => { $('trace-viewed').replaceChildren(); };

  function payloadBlock(span, r) {
    const plain = ui.el('pre', { class: 'plain' });
    for (const p of r.payloads) {
      plain.append(ui.el('strong', { text: p.role + ' (key ' + p.key_id + ')' }));
      let text = p.text;
      if (text == null) text = '(not opened: this session has no key for it' + (p.key_created_at ? '; key ' + p.key_id + ' was made ' + new Date(p.key_created_at * ui.MS).toLocaleDateString() + (p.key_retired_at ? ' and retired ' + new Date(p.key_retired_at * ui.MS).toLocaleDateString() : '') : '') + '. Enter the passphrase it was made with.)';
      plain.append(document.createTextNode('\n' + text + '\n\n'));
    }
    if (!r.payloads.length) plain.textContent = 'Nothing is sealed on this item.';
    return ui.el('div', { class: 'viewed', 'data-span': span.id }, ui.el('h4', { text: span.kind + ' · ' + span.name }), plain);
  }

  function sealedError(text) {
    const line = $('trace-sealed-error');
    line.textContent = text || '';
    show(line, !!text);
  }

  // Sealed: the status bar's Unseal box is the one place the passphrase is typed (app.js). The item is remembered and
  // opens as soon as the session does.
  function askToUnseal() {
    const button = $('seal-button');
    if (button && !button.hidden && button.getAttribute('aria-haspopup') === 'dialog') button.click();
  }

  async function viewSpan(span) {
    sealedError('');
    if (!session.open) {
      state.pending = span;
      sealedError('Sealed. Enter the audit passphrase to open it.');
      askToUnseal();
      return;
    }
    try {
      const r = await api('GET', '/api/admin/v1/audit/payloads/' + encodeURIComponent(span.id));
      const box = $('trace-viewed');
      for (const old of box.querySelectorAll('[data-span="' + CSS.escape(span.id) + '"]')) old.remove();
      box.append(payloadBlock(span, r));
    } catch (e) {
      if (e.message === 'Signed out') return;
      loadSession();
      sealedError(e.message);
    }
  }

  // ------------------------------------------------------------ the session

  // The session is the admin's own (ADR 0011). The status poll says when it ends on its own (a timeout, a sign-out
  // elsewhere, a restart); app.js calls these after the Unseal box and the Seal button.
  function setOpen(open) {
    const was = session.open;
    session.open = !!open;
    if (!session.open) { wipe(); }
    sealedNote();
    if (session.open && !was && state.pending) {
      const span = state.pending;
      state.pending = null;
      sealedError('');
      viewSpan(span);
    }
  }
  function sessionFromStatus(s) { setOpen(!!s.open); }
  async function loadSession() {
    try { setOpen(!!(await api('GET', '/api/admin/v1/audit/session')).open); } catch (_) { /* the status poll reports it */ }
  }

  // ------------------------------------------------------------ opening and closing the panel

  function onClose() {
    state.selected = null;
    state.trace = null;
    state.pending = null;
    state.seq++;
    markSelected();
    wipe();
    sealedError('');
  }

  // Opens one call's trace in the docked panel. ui.openPanel keeps ?id= in the address (a new history entry, so Back
  // closes it); with another trace already open the address is replaced instead.
  async function openTrace(id, opts = {}) {
    const wasOpen = ui.panelOpen() === PANEL;
    const panel = await ui.openPanel(PANEL, { param: 'id', value: id, fromUrl: !!opts.fromUrl, onClose });
    if (!panel) return;
    if (wasOpen) setAddress((p) => p.set('id', id));
    state.selected = id;
    state.pending = null;
    markSelected();
    wipe();
    sealedError('');
    const seq = ++state.seq;
    show($('trace-content'), false);
    show($('trace-error'), false);
    show($('trace-loading'), true);
    let trace;
    try {
      trace = await api('GET', '/api/admin/v1/audit/traces/' + encodeURIComponent(id));
    } catch (e) {
      if (seq !== state.seq || e.message === 'Signed out') return;
      show($('trace-loading'), false);
      $('trace-error').textContent = e.status === 404 ? 'This call is not in the audit trail. It may be older than the time records are kept.' : ui.explain(e);
      show($('trace-error'), true);
      return;
    }
    if (seq !== state.seq) return;
    state.trace = trace;
    show($('trace-loading'), false);
    renderMeta(trace);
    renderWaterfall(trace);
    renderSealed(trace);
    show($('trace-content'), true);
  }

  // ------------------------------------------------------------ loading the page

  function initFromAddress() {
    const q = new URLSearchParams(location.search);
    const status = q.get('status') || '';
    if ([...$('calls-status').options].some((o) => o.value === status)) { state.status = status; $('calls-status').value = status; }
    state.provider = q.get('provider') || '';
    // An old address, /admin/audit/?view=trace&id=..., names the trace the same way.
    return q.get('id');
  }

  let started = false;
  async function load() {
    if (started) return;
    started = true;
    const id = initFromAddress();
    if (id) openTrace(id, { fromUrl: true });
    loadSession();
    await live.start();
  }

  window.askbarAudit = { layout, openTrace, sessionFromStatus, loadSession, loadAccess() {}, session, state, renderKpis };
  registerTab({ id: 'audit/calls', load });
})();
