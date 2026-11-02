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

  async function load() {
    setSub(state.sub);
    await read('/admin/api/audit/stats');
  }

  $('audit-nav').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-sub]');
    if (b) setSub(b.dataset.sub);
  });

  window.askbarAudit = { setSub, read, banner, state };
  registerTab({ id: 'audit', label: 'Audit', load });
})();
