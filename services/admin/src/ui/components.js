// Askbar admin: the shared component builders (plan step 1.9). Loads before app.js and audit.js and
// exposes `window.ui`. The server-rendered half of each component is a macro in templates/components.html;
// the parts built from API data are built here, with DOM calls and textContent only (the CSP sets Trusted
// Types to 'none', so nothing here may assign HTML). No limit or interval is typed here: they come from
// /admin/constants.json, generated from services/admin/src/constants.rs, through `ui.constants`.
(() => {
  'use strict';

  // The one unit conversion the scripts use: a value in seconds times this is milliseconds.
  const MS = 1000;
  const $ = (id) => document.getElementById(id);

  const ui = { constants: null, MS };
  // Every script waits for this before it reads ui.constants.
  ui.ready = fetch('/admin/constants.json', { credentials: 'same-origin' })
    .then((r) => {
      if (!r.ok) throw new Error('constants ' + r.status);
      return r.json();
    })
    .then((c) => {
      ui.constants = Object.freeze(c);
      return ui.constants;
    });
  const secs = (name) => ui.constants[name] * MS;

  // ------------------------------------------------------------- DOM

  // el('button', { class: 'btn', type: 'button', text: 'Save', onclick: fn }, child, 'text', ...)
  // `text` sets textContent, `on<event>` adds a listener, `hidden` a boolean; everything else is an attribute.
  function el(tag, attrs, ...children) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === null || v === undefined || v === false) continue;
      if (k === 'text') node.textContent = v;
      else if (k === 'hidden') node.hidden = true;
      else if (k.startsWith('on') && typeof v === 'function') node.addEventListener(k.slice(2), v);
      else node.setAttribute(k, v === true ? '' : String(v));
    }
    for (const c of children) if (c !== null && c !== undefined && c !== false) node.append(c);
    return node;
  }
  ui.el = el;

  // button(label, { kind: 'primary'|'secondary'|'danger', size: 's'|'l', type, onClick })
  ui.button = (label, opts = {}) => el('button', {
    type: opts.type || 'button',
    class: 'btn btn-' + (opts.kind || 'secondary') + (opts.size ? ' btn-' + opts.size : ''),
    text: label,
    onclick: opts.onClick,
  });

  // The same markup as the status(state) macro: a symbol and a word, never colour alone (rule 37).
  // The symbol is drawn in CSS for the script-built copy (.status-sym::before), so no icon markup is copied here.
  ui.status = (state, word) => el('span', { class: 'status status-' + state },
    el('span', { class: 'status-sym', 'aria-hidden': 'true' }), el('span', { class: 'status-word', text: word }));

  // The same markup as the notice(kind, text, fix) macro, without the icon.
  ui.notice = (kind, text, fix) => {
    const kindWord = { error: 'Error', warning: 'Warning' }[kind] || 'Note';
    return el('div', { class: 'notice notice-' + kind, role: kind === 'error' ? 'alert' : 'status' },
      el('div', { class: 'notice-body' },
        el('p', { class: 'notice-text' }, el('span', { class: 'visually-hidden', text: kindWord + ': ' }), text),
        fix ? el('p', { class: 'notice-fix', text: fix }) : null));
  };

  // The same markup as the empty(text, action) macro; onAction makes the button.
  ui.empty = (text, action, onAction) => el('div', { class: 'empty-state' },
    el('p', { text }),
    action ? el('button', { type: 'button', class: 'btn btn-secondary btn-s', 'data-empty-action': true, text: action, onclick: onAction }) : null);

  // ------------------------------------------------------------- announcer, toasts, banners

  // One announcer (rule 45): polite by default, assertive for critical alerts and banners. Focus stays put.
  ui.announce = (text, opts = {}) => {
    const region = $(opts.urgent ? 'announcer-urgent' : 'announcer');
    if (!region) return;
    // Clear, then set on the next frame, so the same sentence twice is read twice.
    region.textContent = '';
    requestAnimationFrame(() => { region.textContent = text; });
  };

  // Success and Undo only (rule 32); errors are inline. Gone after TOAST_SECS, held while hovered or focused.
  ui.toast = (text, opts = {}) => {
    const region = $('toasts');
    if (!region) return null;
    const toast = el('div', { class: 'toast' }, el('span', { class: 'toast-text', text }));
    let timer = null;
    const dismiss = () => { clearTimeout(timer); toast.remove(); };
    const arm = () => { clearTimeout(timer); timer = setTimeout(dismiss, secs('TOAST_SECS')); };
    if (opts.undo) {
      toast.append(el('button', {
        type: 'button', class: 'btn btn-secondary btn-s', text: 'Undo',
        onclick: () => { dismiss(); opts.undo(); },
      }));
    }
    toast.addEventListener('mouseenter', () => clearTimeout(timer));
    toast.addEventListener('mouseleave', arm);
    toast.addEventListener('focusin', () => clearTimeout(timer));
    toast.addEventListener('focusout', arm);
    region.append(toast);
    ui.announce(text);
    arm();
    return toast;
  };

  // Page-wide banners (rule 39): one per condition, keyed, no close button; they clear when the
  // condition ends (ui.banner(key, kind, null)). Read out assertively. `text` is a sentence, or a list of
  // parts where {code: 'cmd'} is shown as code; everything is set as text. opts.action = {label, onclick}
  // adds one fix button. opts.live updates a banner that is already up in place, without a new announcement
  // (a countdown), so a focused fix button stays focused.
  const bannerText = (parts) => parts.map((p) => (typeof p === 'string' ? p : p.code)).join('');
  const bannerParts = (parts) => parts.map((p) => (typeof p === 'string' ? p : el('code', { text: p.code })));
  ui.banner = (key, kind, text, opts = {}) => {
    const region = $('banners');
    if (!region) return null;
    const old = region.querySelector('[data-banner="' + CSS.escape(key) + '"]');
    const parts = Array.isArray(text) ? text : [text];
    const flat = text ? bannerText(parts) : '';
    if (!kind || !flat) { if (old) old.remove(); return null; }
    if (old && old.dataset.kind === kind) {
      if (old.dataset.text === flat) return old;
      if (opts.live) {
        old.querySelector('.banner-text').textContent = flat;
        old.dataset.text = flat;
        return old;
      }
    }
    const banner = el('div', { class: 'banner banner-' + kind, 'data-banner': key, 'data-kind': kind, 'data-text': flat },
      el('p', { class: 'banner-text' }, ...bannerParts(parts)));
    if (opts.action) {
      banner.append(el('button', { type: 'button', class: 'btn btn-secondary btn-s', 'data-banner-action': true, text: opts.action.label, onclick: opts.action.onclick }));
    }
    if (old) old.replaceWith(banner); else region.append(banner);
    ui.announce(flat, { urgent: true });
    return banner;
  };

  // ------------------------------------------------------------- forms

  // ui.fieldError('host', 'Not a host name.') marks the field and shows the line under it; '' clears it.
  ui.fieldError = (id, text) => {
    const input = $(id);
    const line = $(id + '-error');
    if (input) {
      if (text) input.setAttribute('aria-invalid', 'true'); else input.removeAttribute('aria-invalid');
    }
    if (line) { line.textContent = text || ''; line.hidden = !text; }
    return input;
  };

  // The length rule under a new password or passphrase becomes a live count as you type (rule 24).
  const charCount = (s) => [...s].length;
  ui.lengthHint = (input, min) => {
    const hint = input.parentElement.querySelector('[data-length-hint]') || $(input.id + '-hint');
    if (!hint) return;
    const rule = 'At least ' + min + ' characters';
    const update = () => {
      const n = charCount(input.value);
      hint.classList.toggle('length-ok', n >= min);
      hint.textContent = n === 0 ? rule : n >= min ? n + ' characters' : n + ' of at least ' + min + ' characters';
    };
    input.addEventListener('input', update);
    update();
  };

  // A failed save (rule 26): each field gets its error line, a summary at the top of the form lists them as
  // links, and focus moves to the first field with a problem. ui.formErrors(form, []) clears them all.
  // errors = [{ id, text }]. A field without an error line (the field macro writes one) gets one.
  ui.formErrors = (form, errors) => {
    const old = form.querySelector('.error-summary');
    if (old) old.remove();
    for (const f of form.querySelectorAll('[aria-invalid="true"]')) ui.fieldError(f.id, '');
    if (!errors.length) return;
    const list = el('ul');
    for (const e of errors) {
      const input = $(e.id);
      if (!input) continue;
      if (!$(e.id + '-error')) {
        input.after(el('p', { class: 'field-error error', id: e.id + '-error', hidden: true }));
        const by = input.getAttribute('aria-describedby');
        input.setAttribute('aria-describedby', (by ? by + ' ' : '') + e.id + '-error');
      }
      ui.fieldError(e.id, e.text);
      list.append(el('li', {}, el('a', { href: '#' + e.id, text: e.text, onclick: (ev) => { ev.preventDefault(); input.focus(); } })));
    }
    const summary = el('div', { class: 'error-summary', role: 'alert' },
      el('p', { class: 'error-summary-title', text: errors.length === 1 ? 'Fix this to save:' : 'Fix these ' + errors.length + ' things to save:' }),
      list);
    const heading = form.firstElementChild && /^H[1-6]$/.test(form.firstElementChild.tagName) ? form.firstElementChild : null;
    if (heading) heading.after(summary); else form.prepend(summary);
    const first = $(errors[0].id);
    if (first) first.focus();
  };

  // The values of a form's fields, by id (or name). Password fields count as changed once typed in.
  function fieldValues(root) {
    const out = new Map();
    for (const f of root.querySelectorAll('input, select, textarea')) {
      const key = f.id || f.name;
      if (!key || f.type === 'submit' || f.type === 'button') continue;
      out.set(key, f.type === 'checkbox' || f.type === 'radio' ? f.checked : f.value);
    }
    return out;
  }
  function changedFields(root, before) {
    const now = fieldValues(root);
    return [...now.keys()].filter((k) => before.has(k) && before.get(k) !== now.get(k));
  }
  function labelFor(root, id) {
    const label = root.querySelector('label[for="' + CSS.escape(id) + '"]');
    if (!label) return id;
    const copy = label.cloneNode(true);
    for (const opt of copy.querySelectorAll('.optional')) opt.remove();
    return copy.textContent.trim();
  }

  // The save bar (rule 25): shows once something changes, "1 unsaved change: Host name", with Discard and Save.
  // Returns { dirty(), saved() }; call saved() after a successful save so the current values become the base.
  ui.trackChanges = (form) => {
    const bar = $(form.id + '-save-bar');
    let base = fieldValues(form);
    const text = bar && bar.querySelector('[data-save-text]');
    const update = () => {
      const changed = changedFields(form, base);
      if (!bar) return;
      bar.hidden = changed.length === 0;
      if (text) {
        text.textContent = changed.length === 1
          ? '1 unsaved change: ' + labelFor(form, changed[0])
          : changed.length + ' unsaved changes';
      }
    };
    form.addEventListener('input', update);
    form.addEventListener('change', update);
    const discard = bar && bar.querySelector('[data-save-discard]');
    if (discard) {
      discard.addEventListener('click', () => {
        for (const f of form.querySelectorAll('input, select, textarea')) {
          const key = f.id || f.name;
          if (!base.has(key)) continue;
          if (f.type === 'checkbox' || f.type === 'radio') f.checked = base.get(key); else f.value = base.get(key);
        }
        update();
      });
    }
    update();
    return {
      dirty: () => changedFields(form, base).length > 0,
      saved: () => { base = fieldValues(form); update(); },
    };
  };

  // A saving button: says what it is doing and ignores clicks until done() (rule 26's summary is the caller's).
  // aria-disabled, not disabled, so keyboard focus stays on it.
  ui.busy = (button, text) => {
    const label = button.textContent;
    const block = (e) => { e.preventDefault(); e.stopImmediatePropagation(); };
    button.textContent = text;
    button.setAttribute('aria-busy', 'true');
    button.setAttribute('aria-disabled', 'true');
    button.addEventListener('click', block, true);
    if (button.form) button.form.addEventListener('submit', block, true);
    return () => {
      button.textContent = label;
      button.removeAttribute('aria-busy');
      button.removeAttribute('aria-disabled');
      button.removeEventListener('click', block, true);
      if (button.form) button.form.removeEventListener('submit', block, true);
    };
  };

  // ------------------------------------------------------------- boxes and confirmations

  // A box (rule 29) is a modal <dialog> from the dialog_box macro. The browser returns focus on close.
  ui.openBox = (id) => {
    const box = typeof id === 'string' ? $(id) : id;
    if (box && !box.open) box.showModal();
    return box;
  };
  ui.closeBox = (id) => {
    const box = typeof id === 'string' ? $(id) : id;
    if (box && box.open) box.close();
  };

  // ui.confirm({ title, body, action, kind, typeName }) resolves true when confirmed. With typeName the action
  // waits until the name is typed (rule 31). With alt = { text, label, onClick } a gentler way out is offered above the
  // form ("Might Sam need it back? [Pause instead]"): it closes the box as a refusal and then runs onClick.
  // Built like the dialog_box macro; removed when closed.
  ui.confirm = (opts) => new Promise((resolve) => {
    const id = 'confirm-' + Date.now().toString(36);
    const title = el('h2', { class: 'box-title', id: id + '-title', text: opts.title });
    const body = el('div', { class: 'box-body' });
    if (opts.body) body.append(el('p', { text: opts.body }));
    const alt = opts.alt
      ? el('p', { class: 'hint' }, opts.alt.text + ' ',
        el('button', { type: 'button', class: 'btn btn-secondary btn-s', 'data-confirm-alt': true, text: opts.alt.label }))
      : null;
    if (alt) body.append(alt);
    const go = el('button', { type: 'submit', class: 'btn btn-' + (opts.kind || 'danger'), text: opts.action, 'data-confirm-action': true });
    const cancel = el('button', { type: 'button', class: 'btn btn-secondary', text: opts.cancel || 'Cancel', 'data-confirm-cancel': true });
    const form = el('form', { method: 'dialog' });
    let typed = null;
    if (opts.typeName) {
      typed = el('input', { type: 'text', id: id + '-name', autocomplete: 'off', spellcheck: 'false' });
      form.append(el('div', { class: 'field' },
        el('label', { for: id + '-name', text: 'Type ' + opts.typeName + ' to confirm' }), typed));
      go.disabled = true;
      typed.addEventListener('input', () => { go.disabled = typed.value !== opts.typeName; });
    }
    form.append(el('div', { class: 'actions' }, cancel, go));
    body.append(form);
    const box = el('dialog', { class: 'box', id, 'aria-labelledby': id + '-title' }, title, body);
    let result = false;
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      if (typed && typed.value !== opts.typeName) return;
      result = true;
      box.close();
    });
    cancel.addEventListener('click', () => box.close());
    if (alt) alt.querySelector('button').addEventListener('click', () => { box.close(); opts.alt.onClick(); });
    box.addEventListener('close', () => { box.remove(); resolve(result); });
    document.body.append(box);
    box.showModal();
    (typed || cancel).focus();
  });

  // Inline row confirmation (rule 30): the row opens with the question, Cancel and the action at its
  // bottom right; the row's own button hides until it closes. ui.rowConfirm(row, { text, action, trigger,
  // onConfirm }); onConfirm may be async. A table row gets a full-width row under it.
  ui.rowConfirm = (row, opts) => {
    const trigger = opts.trigger || null;
    const cancel = el('button', { type: 'button', class: 'btn btn-secondary btn-s', 'data-row-cancel': true, text: 'Cancel' });
    const go = el('button', { type: 'button', class: 'btn btn-danger btn-s', 'data-row-action': true, text: opts.action });
    const block = el('div', { class: 'row-confirm', role: 'group', 'aria-label': opts.text },
      el('p', { class: 'row-confirm-text', text: opts.text }), el('div', { class: 'row-confirm-actions' }, cancel, go));
    let holder = block;
    if (row.tagName === 'TR') {
      holder = el('tr', { class: 'row-confirm-row' }, el('td', { colspan: String(row.cells.length || 1) }, block));
      row.after(holder);
    } else {
      row.append(block);
    }
    if (trigger) trigger.hidden = true;
    const close = () => {
      holder.remove();
      if (trigger) { trigger.hidden = false; trigger.focus(); }
    };
    cancel.addEventListener('click', close);
    holder.addEventListener('keydown', (e) => { if (e.key === 'Escape') { e.preventDefault(); close(); } });
    go.addEventListener('click', async () => {
      const done = ui.busy(go, opts.busy || opts.action);
      try { await opts.onConfirm(); close(); } catch (err) { done(); throw err; }
    });
    cancel.focus();
    return { close };
  };

  // ------------------------------------------------------------- side panel

  // One docked side panel at a time (rules 27, 28), from the side_panel macro. The page narrows (body.panel-open).
  // It has its own address (?panel=<id>, or opts.param=value), so Back closes it; Esc and × close it; unsaved input
  // asks first. ui.openPanel(id, { param, value, onClose }) resolves to the panel, or null if the open one stayed.
  let openState = null; // { panel, opener, before, onClose, pushed }
  let ignorePop = false;

  function panelUrl(param, value) {
    const u = new URL(location.href);
    u.searchParams.set(param, value);
    return u.pathname + u.search + u.hash;
  }
  function urlWithout(param) {
    const u = new URL(location.href);
    u.searchParams.delete(param);
    return u.pathname + u.search + u.hash;
  }

  ui.openPanel = async (id, opts = {}) => {
    const panel = $(id);
    if (!panel) return null;
    if (openState && openState.panel === panel) return panel;
    if (openState && !(await ui.closePanel())) return null;
    const param = opts.param || 'panel';
    const value = opts.value || id;
    openState = {
      panel, param,
      opener: document.activeElement,
      before: fieldValues(panel),
      onClose: opts.onClose || null,
      pushed: !opts.fromUrl,
    };
    if (openState.pushed) history.pushState({ panel: id }, '', panelUrl(param, value));
    panel.hidden = false;
    document.body.classList.add('panel-open');
    const title = panel.querySelector('.side-panel-title');
    if (title) { title.tabIndex = -1; title.focus(); }
    return panel;
  };

  // Resolves true once closed, false if the admin chose to keep unsaved input.
  ui.closePanel = async (opts = {}) => {
    if (!openState) return true;
    const state = openState;
    if (!opts.force && changedFields(state.panel, state.before).length > 0) {
      const discard = await ui.confirm({
        title: 'Discard your changes?', body: 'What you typed in this panel is not saved.',
        action: 'Discard', cancel: 'Keep editing',
      });
      if (!discard) {
        // Back already left the panel's address; put it back.
        if (opts.fromHistory) history.pushState({ panel: state.panel.id }, '', panelUrl(state.param, state.panel.id));
        return false;
      }
    }
    openState = null;
    state.panel.hidden = true;
    document.body.classList.remove('panel-open');
    if (!opts.fromHistory) {
      if (state.pushed && history.state && history.state.panel === state.panel.id) { ignorePop = true; history.back(); }
      else history.replaceState(null, '', urlWithout(state.param));
    }
    if (state.onClose) state.onClose();
    if (state.opener && state.opener.isConnected && typeof state.opener.focus === 'function') state.opener.focus();
    return true;
  };
  ui.panelOpen = () => (openState ? openState.panel.id : null);

  window.addEventListener('popstate', () => {
    if (ignorePop) { ignorePop = false; return; }
    if (openState && !(history.state && history.state.panel === openState.panel.id)) ui.closePanel({ fromHistory: true });
  });

  document.addEventListener('click', (e) => {
    const close = e.target.closest('[data-panel-close]');
    if (close && openState && openState.panel.contains(close)) ui.closePanel();
  });

  // ------------------------------------------------------------- menus

  // ui.menu(button, items) opens a popup menu under the button (rule 33): items = [{ label, action, danger }],
  // Delete last. With a function instead of items it drives the menu already next to the button (row_menu
  // macro) and calls the function with the item's data-action. Arrow keys, Home, End, Esc and Tab work.
  ui.menu = (button, items) => {
    let menu = button.nextElementSibling && button.nextElementSibling.classList.contains('menu') ? button.nextElementSibling : null;
    let onAction = typeof items === 'function' ? items : null;
    if (Array.isArray(items)) {
      if (menu) menu.remove();
      menu = el('div', { class: 'menu', role: 'menu', hidden: true });
      const byLabel = new Map();
      for (const item of items) {
        const b = el('button', { type: 'button', class: 'menu-item' + (item.danger ? ' menu-item-danger' : ''), role: 'menuitem', text: item.label });
        byLabel.set(b, item.action);
        menu.append(b);
      }
      onAction = (_, b) => { const fn = byLabel.get(b); if (fn) fn(); };
      button.after(menu);
      if (button.parentElement) button.parentElement.classList.add('menu-anchor');
    }
    if (!menu) return null;
    if (!menu.id) menu.id = 'menu-' + Math.random().toString(36).slice(2);
    button.setAttribute('aria-haspopup', 'menu');
    button.setAttribute('aria-controls', menu.id);
    button.setAttribute('aria-expanded', 'false');
    const entries = () => [...menu.querySelectorAll('[role="menuitem"]')];
    for (const b of entries()) b.tabIndex = -1;

    const outside = (e) => { if (!menu.contains(e.target) && e.target !== button && !button.contains(e.target)) close(false); };
    function open(focusLast) {
      menu.hidden = false;
      button.setAttribute('aria-expanded', 'true');
      const list = entries();
      const target = focusLast ? list[list.length - 1] : list[0];
      if (target) target.focus();
      document.addEventListener('pointerdown', outside, true);
    }
    function close(refocus) {
      if (menu.hidden) return;
      menu.hidden = true;
      button.setAttribute('aria-expanded', 'false');
      document.removeEventListener('pointerdown', outside, true);
      if (refocus) button.focus();
    }
    button.addEventListener('click', () => (menu.hidden ? open(false) : close(true)));
    button.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); open(e.key === 'ArrowUp'); }
    });
    menu.addEventListener('keydown', (e) => {
      const list = entries();
      const at = list.indexOf(document.activeElement);
      const move = (i) => { e.preventDefault(); list[(i + list.length) % list.length].focus(); };
      if (e.key === 'ArrowDown') move(at + 1);
      else if (e.key === 'ArrowUp') move(at - 1);
      else if (e.key === 'Home') move(0);
      else if (e.key === 'End') move(list.length - 1);
      else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(true); }
      else if (e.key === 'Tab') close(false);
    });
    menu.addEventListener('click', (e) => {
      const item = e.target.closest('[role="menuitem"]');
      if (!item) return;
      close(true);
      if (onAction) onAction(item.dataset.action, item);
    });
    return { menu, open, close };
  };

  // ------------------------------------------------------------- alerts bell (plan step 3.4, rules 38, 45)

  // ui.bell(items) draws the status bar's bell from a list of { key, severity: 'critical'|'warning'|'info', text,
  // href, fix (the page's name), at (Unix seconds, optional) }. The badge counts critical plus warning and is hidden
  // at 0; the button's name says the counts. The menu groups Critical, Warning, Info; each item is a link to where it
  // is fixed. "See all alerts" links to the Alerts page (step 4.4).
  const SEVERITIES = [['critical', 'Critical'], ['warning', 'Warning'], ['info', 'Info']];
  // The severity icons are server-rendered into #alert-icons (components.html icon macro); copy, never build HTML.
  const alertIcon = (name) => {
    const holder = $('alert-icons');
    const src = holder && holder.content.querySelector('[data-icon="' + name + '"]');
    return src && src.firstElementChild ? src.firstElementChild.cloneNode(true) : null;
  };
  ui.alertIcon = alertIcon;
  let bell = null;
  let bellSignature = '';
  ui.bell = (items) => {
    const button = $('bell-button');
    if (!button) return;
    if (!bell) bell = ui.menu(button, () => {});
    const count = (sev) => items.filter((i) => i.severity === sev).length;
    const crit = count('critical');
    const warn = count('warning');
    const parts = SEVERITIES.map(([sev]) => [count(sev), sev]).filter(([n]) => n > 0).map(([n, sev]) => n + ' ' + sev);
    const name = 'Alerts: ' + (parts.length ? parts.join(', ') : 'none');
    button.setAttribute('aria-label', name);
    button.dataset.tip = name;
    const badge = $('bell-badge');
    badge.hidden = crit + warn === 0;
    badge.textContent = String(crit + warn);
    badge.classList.toggle('bell-badge-critical', crit > 0);
    const signature = JSON.stringify(items.map((i) => [i.key, i.severity, i.text, i.href, i.at || 0]));
    if (signature === bellSignature) return;
    bellSignature = signature;

    const menu = bell.menu;
    const inside = menu.contains(document.activeElement);
    const was = inside ? [...menu.querySelectorAll('[role="menuitem"]')].indexOf(document.activeElement) : -1;
    menu.replaceChildren();
    if (!items.length) menu.append(el('span', { class: 'bell-empty', role: 'menuitem', 'aria-disabled': 'true', tabindex: '-1', text: 'No alerts' }));
    for (const [sev, word] of SEVERITIES) {
      const group = items.filter((i) => i.severity === sev);
      if (!group.length) continue;
      menu.append(el('div', { role: 'group', 'aria-label': word + ' alerts', class: 'bell-group' },
        el('div', { class: 'bell-group-label bell-' + sev, 'aria-hidden': 'true', text: word + ' · ' + group.length }),
        ...group.map((i) => el('a', { role: 'menuitem', class: 'bell-item bell-' + sev, href: i.href, tabindex: '-1', 'data-alert': i.key },
          el('span', { class: 'bell-icon' }, alertIcon(sev)),
          el('span', { class: 'bell-text' }, el('span', { text: i.text }),
            el('small', {}, i.fix, i.at ? ' · ' : '', i.at ? ui.time(i.at) : null))))));
    }
    const all = el('a', { role: 'menuitem', class: 'bell-all', href: '/admin/alerts/', tabindex: '-1', text: 'See all alerts' });
    menu.append(all);
    if (was >= 0) {
      const list = [...menu.querySelectorAll('[role="menuitem"]')];
      list[Math.min(was, list.length - 1)].focus();
    }
  };

  // A critical alert pops up beside the bell the first time it appears (rule 38). It is not a toast (rule 32: toasts
  // are success and Undo only): its own .alert-pop in #alert-pops, held until it is closed or the condition ends.
  // It is read out assertively and never takes focus. Returns the element; ui.alertPopClear(key) removes it.
  ui.alertPop = (alert) => {
    const region = $('alert-pops');
    if (!region || region.querySelector('[data-alert-pop="' + CSS.escape(alert.key) + '"]')) return null;
    const close = el('button', { type: 'button', class: 'btn btn-quiet btn-icon', 'aria-label': 'Close alert', 'data-tip': 'Close', onclick: () => pop.remove() }, alertIcon('close'));
    const pop = el('div', { class: 'alert-pop', role: 'group', 'aria-label': 'Critical alert', 'data-alert-pop': alert.key },
      el('span', { class: 'alert-pop-icon' }, alertIcon('critical')),
      el('div', { class: 'alert-pop-body' },
        el('span', { class: 'alert-pop-sev', text: 'Critical' }),
        el('p', { class: 'alert-pop-text', text: alert.text }),
        el('a', { class: 'btn btn-secondary btn-s', href: alert.href, text: 'Open ' + alert.fix })),
      close);
    region.append(pop);
    ui.announce('Critical alert: ' + alert.text, { urgent: true });
    return pop;
  };
  ui.alertPopClear = (key) => {
    const region = $('alert-pops');
    const pop = region && region.querySelector('[data-alert-pop="' + CSS.escape(key) + '"]');
    if (pop) pop.remove();
  };

  // ------------------------------------------------------------- combobox (plan step 3.5, rule 4)

  // ui.highlight(text, query) returns a fragment of `text` with every query word wrapped in <mark>, built as nodes.
  ui.highlight = (text, query) => {
    const frag = document.createDocumentFragment();
    const lower = text.toLowerCase();
    const ranges = [];
    for (const word of query.toLowerCase().split(/\s+/).filter(Boolean)) {
      const at = lower.indexOf(word);
      if (at >= 0) ranges.push([at, at + word.length]);
    }
    ranges.sort((a, b) => a[0] - b[0]);
    const merged = [];
    for (const r of ranges) {
      const last = merged[merged.length - 1];
      if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1]);
      else merged.push([r[0], r[1]]);
    }
    let at = 0;
    for (const [from, to] of merged) {
      if (from > at) frag.append(text.slice(at, from));
      frag.append(el('mark', { text: text.slice(from, to) }));
      at = to;
    }
    if (at < text.length) frag.append(text.slice(at));
    return frag;
  };

  // ui.combobox({ input, list, pop, none, groups(query), icon(item), choose(item, event) }) is the ARIA 1.2 combobox
  // with a listbox: the input keeps focus and names the highlighted option in aria-activedescendant. groups(query)
  // returns [{ key, label, items: [{ title, where, value, href, ... }] }]; a group with no items is not drawn.
  // Arrows move, Enter chooses (the first row when none is highlighted), Esc closes and a second Esc clears.
  // Returns { render, close }. The caller shows `pop` (the popup holding `list` and `none`) through it.
  ui.combobox = (o) => {
    const { input, list, pop, none } = o;
    let rows = [];
    let active = -1;
    let open = false;
    const setOpen = (v) => {
      open = v;
      pop.hidden = !v;
      input.setAttribute('aria-expanded', String(v));
      if (!v) input.removeAttribute('aria-activedescendant');
    };
    const select = (i) => {
      if (active >= 0 && rows[active]) rows[active].node.setAttribute('aria-selected', 'false');
      active = i;
      if (i < 0 || !rows[i]) { input.removeAttribute('aria-activedescendant'); return; }
      rows[i].node.setAttribute('aria-selected', 'true');
      input.setAttribute('aria-activedescendant', rows[i].node.id);
      rows[i].node.scrollIntoView({ block: 'nearest' });
    };
    const render = () => {
      const query = input.value.trim();
      list.textContent = '';
      rows = [];
      active = -1;
      none.hidden = true;
      if (!query) { setOpen(false); return; }
      for (const g of o.groups(query)) {
        if (!g.items.length) continue;
        const heading = el('div', { class: 'gs-group', id: list.id + '-h-' + g.key, text: g.label });
        const group = el('div', { role: 'group', 'aria-labelledby': heading.id }, heading);
        for (const item of g.items) {
          const node = el('a', { class: 'gs-row', role: 'option', id: list.id + '-o-' + rows.length, href: item.href, tabindex: '-1', 'aria-selected': 'false' },
            o.icon ? o.icon(item) : null,
            el('span', { class: 'gs-main' },
              el('span', { class: 'gs-title' }, ui.highlight(item.title, query)),
              item.where ? el('span', { class: 'gs-where', text: item.where }) : null),
            item.value ? el('span', { class: 'gs-val', text: item.value }) : null);
          rows.push({ node, item });
          group.append(node);
        }
        list.append(group);
      }
      none.hidden = rows.length > 0;
      setOpen(true);
      if (rows.length) select(0);
      ui.announce(rows.length ? rows.length + (rows.length === 1 ? ' result' : ' results') : 'No matches');
    };
    const choose = (item, e) => { setOpen(false); o.choose(item, e); };
    input.addEventListener('input', render);
    input.addEventListener('focus', () => { if (input.value.trim() && !open) render(); });
    input.addEventListener('blur', () => setOpen(false));
    input.addEventListener('keydown', (e) => {
      const n = rows.length;
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        if (!open) { render(); return; }
        if (n) select((active + (e.key === 'ArrowDown' ? 1 : n - 1)) % n);
      } else if (e.key === 'Enter') {
        if (!open || !n) return;
        e.preventDefault();
        choose(rows[active >= 0 ? active : 0].item, e);
      } else if (e.key === 'Escape') {
        if (open) { e.preventDefault(); e.stopPropagation(); setOpen(false); }
        else if (input.value) { e.preventDefault(); e.stopPropagation(); input.value = ''; render(); }
      } else if (e.key === 'Tab') {
        setOpen(false);
      }
    });
    // A press on the popup must not take focus from the input (it would close the popup before the click lands).
    pop.addEventListener('mousedown', (e) => e.preventDefault());
    pop.addEventListener('mouseover', (e) => {
      const at = rows.findIndex((r) => r.node === e.target.closest('.gs-row'));
      if (at >= 0 && at !== active) select(at);
    });
    pop.addEventListener('click', (e) => {
      const hit = rows.find((r) => r.node === e.target.closest('.gs-row'));
      if (!hit || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey) return; // let "open in a new tab" work
      e.preventDefault();
      choose(hit.item, e);
    });
    return { render, close: () => setOpen(false) };
  };

  // ------------------------------------------------------------- lists

  // ui.rows(container, items, render, opts) merges a keyed list in place (rule 34). `container` is a <tbody> (render
  // returns a <tr>) or any element whose children are the rows. A row is its `key(item)` (default item.id), kept in
  // data-key, and its `sig(item)` (default the JSON of the item), kept in data-sig:
  //   - a key that is still there with the same sig keeps its node, so scroll, focus, selection and typing stay;
  //   - a changed sig builds a new node in its place (focus goes back to the same control, by position);
  //   - a new key is built, and with opts.fresh gets the .row-new class for HIGHLIGHT_SECS and is announced politely
  //     ("1 new client", opts.noun = ['client', 'clients']);
  //   - a key that is gone is removed. opts.keep(node) holds a row as it is (a row with an open confirmation).
  // opts.force rebuilds every row; opts.empty is the sentence shown when there are no items.
  ui.rows = (container, items, render, opts = {}) => {
    const key = opts.key || ((x) => x.id);
    const sigOf = opts.sig || ((x) => JSON.stringify(x));
    const rowOf = (node) => (node && container.contains(node) ? node.closest('[data-key]') : null);
    const active = document.activeElement;
    const activeRow = rowOf(active);
    let focusKey = null;
    let focusIndex = -1;
    const controls = (row) => [...row.querySelectorAll('button, a, input, select, textarea, [tabindex]')];
    if (activeRow) { focusKey = activeRow.dataset.key; focusIndex = controls(activeRow).indexOf(active); }
    const old = new Map([...container.children].filter((c) => c.dataset.key !== undefined).map((c) => [c.dataset.key, c]));
    const rows = [];
    const added = [];
    for (const item of items) {
      const k = String(key(item));
      const sig = sigOf(item);
      const prev = old.get(k);
      if (prev && !opts.force && (prev.dataset.sig === sig || (opts.keep && opts.keep(prev)))) { rows.push(prev); continue; }
      const node = render(item);
      node.dataset.key = k;
      node.dataset.sig = sig;
      rows.push(node);
      if (!prev) added.push(node);
    }
    if (rows.length === 0 && opts.empty) {
      const cols = container.tagName === 'TBODY' && container.closest('table') ? container.closest('table').querySelectorAll('thead th').length : 1;
      rows.push(container.tagName === 'TBODY'
        ? el('tr', { class: 'empty-row' }, el('td', { colspan: String(cols || 1) }, opts.empty))
        : el('p', { class: 'hint empty-row', text: opts.empty }));
    }
    const keep = new Set(rows);
    for (const child of [...container.children]) if (!keep.has(child)) child.remove();
    rows.forEach((row, i) => { if (container.children[i] !== row) container.insertBefore(row, container.children[i] || null); });
    if (focusKey !== null && !(active && active.isConnected)) {
      const again = rows.find((r) => r.dataset && r.dataset.key === focusKey);
      const list = again ? controls(again) : [];
      if (list[focusIndex]) list[focusIndex].focus();
    }
    if (opts.fresh && added.length) {
      for (const row of added) row.classList.add('row-new');
      setTimeout(() => { for (const row of added) row.classList.remove('row-new'); }, secs('HIGHLIGHT_SECS'));
      const noun = opts.noun || ['item', 'items'];
      ui.announce(added.length + ' new ' + noun[added.length === 1 ? 0 : 1]);
    }
    return rows;
  };

  // ui.live(load, opts) keeps a list current (rules 34 and 35). load() fetches and returns the data (the caller's fetch
  // may send If-None-Match and hand back the data it kept on a 304); opts.render(data, { first, force }) draws it.
  // The body is compared as JSON, so an unchanged answer changes nothing in the page (opts.always renders every time).
  //   every     name of the interval constant while the tab is visible (default LIVE_EVERY_SECS)
  //   hidden    'pause' (default: lists wait until the tab is visible again) or 'slow' (HIDDEN_EVERY_SECS: the bell,
  //             the status and the tab title go on)
  //   when()    false while there is nothing to refresh (signed out); skip() holds a check back (a restart is running)
  //   notice    an element: "Reconnecting…" is shown just before it while checks fail; or a function(on) to show it
  //             another way
  //   onError(err) an answer that is not a network failure (a refusal, a 5xx), or a failed render
  //   timer     { set(fn, ms), clear(id) } instead of setTimeout (tests)
  // A network failure backs off along BACKOFF_SECS (the last step repeats) and is not an error to show; the first
  // success clears the notice and goes back to the normal interval. start() loads now and keeps going; refresh() loads
  // now (call it right after the admin's own change); refresh({ force: true }) also rebuilds every row.
  ui.live = (load, opts = {}) => {
    const timer = opts.timer || { set: (fn, ms) => setTimeout(fn, ms), clear: (id) => clearTimeout(id) };
    let id = null;
    let failures = 0;
    let last = null;
    let rendered = false;
    let started = false;
    let note = null;
    let chain = Promise.resolve();
    const state = { delay: null, failures: 0, polls: 0 };

    const reconnecting = (on) => {
      if (typeof opts.notice === 'function') { opts.notice(on); return; }
      if (!opts.notice) return;
      if (on && !note) {
        note = ui.notice('warning', 'Reconnecting…');
        note.classList.add('live-note');
        opts.notice.before(note);
      } else if (!on && note) { note.remove(); note = null; }
    };
    const delay = () => {
      if (failures) { const steps = ui.constants.BACKOFF_SECS; return steps[Math.min(failures - 1, steps.length - 1)] * MS; }
      if (!document.hidden) return secs(opts.every || 'LIVE_EVERY_SECS');
      return opts.hidden === 'slow' ? secs('HIDDEN_EVERY_SECS') : null;
    };
    const schedule = () => {
      if (id !== null) { timer.clear(id); id = null; }
      state.delay = null;
      if (!started) return;
      const ms = delay();
      state.delay = ms;
      if (ms !== null) id = timer.set(tick, ms);
    };
    async function tick() {
      id = null;
      if ((opts.when && !opts.when()) || (opts.skip && opts.skip())) { schedule(); return; }
      state.polls++;
      await refresh();
    }
    async function run(o) {
      let data;
      try {
        data = await load();
      } catch (err) {
        last = null;
        // No answer at all (no status) is a lost connection; an answer, even a refusal or 5xx, is not.
        const lost = err && err.status === undefined && err.message !== 'Signed out';
        failures = lost ? failures + 1 : 0;
        reconnecting(lost);
        state.failures = failures;
        if (opts.onError) opts.onError(err);
        return;
      }
      failures = 0;
      state.failures = 0;
      reconnecting(false);
      const body = JSON.stringify(data);
      if (!o.force && !opts.always && rendered && body === last) return;
      last = body;
      try {
        opts.render(data, { first: !rendered, force: !!o.force });
        rendered = true;
      } catch (err) {
        last = null;
        if (opts.onError) opts.onError(err);
      }
    }
    function refresh(o = {}) {
      chain = chain.then(() => run(o)).then(schedule, schedule);
      return chain;
    }
    const seen = () => { if (started && !document.hidden) refresh(); else schedule(); };
    document.addEventListener('visibilitychange', seen);
    return {
      state,
      refresh,
      start() { if (!started) { started = true; } return refresh(); },
      stop() { started = false; schedule(); document.removeEventListener('visibilitychange', seen); reconnecting(false); },
    };
  };

  // ------------------------------------------------------------- time, numbers, copy

  const UNITS = [['year', 365 * 24 * 3600], ['month', 30 * 24 * 3600], ['day', 24 * 3600], ['hour', 3600], ['minute', 60], ['second', 1]];
  const relative = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
  const full = new Intl.DateTimeFormat(undefined, { dateStyle: 'full', timeStyle: 'long' });

  // A relative time with the full date on hover (rule 36). ts is Unix seconds, as the admin API sends it.
  ui.time = (ts) => {
    const when = new Date(ts * MS);
    const diff = ts - Date.now() / MS;
    const [unit, size] = UNITS.find(([, s]) => Math.abs(diff) >= s) || UNITS[UNITS.length - 1];
    return el('time', { datetime: when.toISOString(), title: full.format(when), text: relative.format(Math.round(diff / size), unit) });
  };

  // A number with its unit, right-aligned in a .num cell (rule 36).
  ui.num = (n, unit) => el('span', { class: 'num', text: Number(n).toLocaleString() + (unit ? ' ' + unit : '') });

  // Copies value (or the text of the element the button's data-copy names) and says so for TOAST_SECS.
  ui.copy = async (button, value) => {
    const source = button.dataset.copy ? $(button.dataset.copy) : null;
    const text = value !== undefined ? value : source ? source.textContent : '';
    const name = button.getAttribute('aria-label') || 'Copy';
    const iconOnly = button.classList.contains('btn-icon');
    const say = (word) => {
      if (iconOnly) { button.setAttribute('aria-label', word); button.dataset.tip = word; } else button.textContent = word;
      ui.announce(word);
      setTimeout(() => {
        if (iconOnly) { button.setAttribute('aria-label', name); button.dataset.tip = name; } else button.textContent = name;
      }, secs('TOAST_SECS'));
    };
    try {
      await navigator.clipboard.writeText(text);
      say('Copied');
      return true;
    } catch (_) {
      // No clipboard (an insecure context or a refusal): select it so Ctrl+C or Cmd+C works.
      if (source) getSelection().selectAllChildren(source);
      say('Press Ctrl+C to copy');
      return false;
    }
  };
  document.addEventListener('click', (e) => {
    const b = e.target.closest('button[data-copy]');
    if (b) ui.copy(b);
  });

  // ------------------------------------------------------------- keys

  // "Skip to content" (rule 41) moves focus to whichever <main> is on screen: the app, sign-in or setup.
  document.addEventListener('click', (e) => {
    const skip = e.target.closest('a.skip-link');
    if (!skip) return;
    const main = [...document.querySelectorAll('main')].find((m) => !m.closest('[hidden]'));
    if (!main) return;
    e.preventDefault();
    if (!main.hasAttribute('tabindex')) main.tabIndex = -1;
    main.focus();
  });

  // Esc closes the side panel when nothing on top of it (a box or a menu) took the key first.
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape' || e.defaultPrevented || !openState) return;
    if (document.querySelector('dialog[open]')) return;
    e.preventDefault();
    ui.closePanel();
  });

  // ------------------------------------------------------------- side menu

  // The collapse button shrinks the menu to icons (rule 2). The choice is this browser's own, so it lives in
  // localStorage; a store that is blocked or throws leaves the menu expanded and the page working. The class
  // is set here, as the page loads and while #app is still hidden, so there is no inline script.
  const MENU_KEY = 'askbar-admin.menu';
  const stored = (() => { try { return localStorage.getItem(MENU_KEY); } catch (_) { return null; } })();
  function setCollapsed(collapsed, remember) {
    document.documentElement.classList.toggle('menu-collapsed', collapsed);
    const b = $('menu-collapse');
    if (b) {
      const name = collapsed ? 'Expand menu' : 'Collapse menu';
      b.setAttribute('aria-expanded', String(!collapsed));
      b.setAttribute('aria-label', name);
      b.dataset.tip = name;
    }
    if (remember) {
      try { localStorage.setItem(MENU_KEY, collapsed ? 'collapsed' : 'expanded'); } catch (_) { /* kept for this page only */ }
    }
  }
  setCollapsed(stored === 'collapsed', false);
  document.addEventListener('click', (e) => {
    if (e.target.closest('#menu-collapse')) setCollapsed(!document.documentElement.classList.contains('menu-collapsed'), true);
  });

  // The menu's names (rules 2, 42): an icon button's name, and every item's while the menu is collapsed, show
  // beside it on hover and on keyboard focus. The menu scrolls, so a tooltip inside it would be clipped: one
  // fixed tooltip outside it is placed next to the item instead. It only repeats the item's accessible name
  // (aria-hidden), and Esc hides it (WCAG 1.4.13).
  const menuTip = el('div', { id: 'menu-tip', class: 'menu-tip', 'aria-hidden': 'true', hidden: true });
  function tipFor(target) {
    const item = target && target.closest ? target.closest('.side-menu [data-tip]') : null;
    if (!item) return null;
    const collapsed = document.documentElement.classList.contains('menu-collapsed');
    return item.classList.contains('btn-icon') || collapsed ? item : null;
  }
  let tipItem = null;
  function showMenuTip(item) {
    if (!menuTip.isConnected) document.body.append(menuTip);
    tipItem = item;
    const r = item.getBoundingClientRect();
    const menuRight = item.closest('.side-menu').getBoundingClientRect().right;
    menuTip.textContent = item.dataset.tip;
    menuTip.hidden = false;
    // Beside the menu; where that leaves the window (a phone's full-width menu), on the item's left instead.
    const gap = parseFloat(getComputedStyle(menuTip).marginLeft) || 0;
    let left = Math.max(r.right, menuRight);
    if (left + gap + menuTip.offsetWidth > document.documentElement.clientWidth) left = r.left - menuTip.offsetWidth - 2 * gap;
    menuTip.style.top = (r.top + r.height / 2) + 'px';
    menuTip.style.left = left + 'px';
  }
  const hideMenuTip = () => { tipItem = null; menuTip.hidden = true; };
  document.addEventListener('mouseover', (e) => { const item = tipFor(e.target); if (item) showMenuTip(item); });
  document.addEventListener('mouseout', (e) => { if (tipFor(e.target) && !tipFor(e.relatedTarget)) hideMenuTip(); });
  document.addEventListener('focusin', (e) => { const item = tipFor(e.target); if (item && item.matches(':focus-visible')) showMenuTip(item); else hideMenuTip(); });
  document.addEventListener('focusout', hideMenuTip);
  // Esc with a tooltip showing only hides the tooltip; the next Esc reaches the menu or the panel.
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape' || menuTip.hidden) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    hideMenuTip();
  });
  // The phone menu (plan step 2.4, rule 2): the Menu button in the phone bar opens the same menu full screen.
  // While it is open the page behind is inert and Tab stays in the menu and its button; Esc or Close shuts it
  // and focus goes back to the button. The button is only shown on a phone, so "is it shown" is the check.
  const phoneOpen = () => document.documentElement.classList.contains('menu-open');
  // The menu's tab stops: its links and buttons, and the search field the phone menu holds. A search result is a link
  // too, but the input keeps focus while it is chosen, so results are not stops (tabindex -1).
  const MENU_LINKS = 'a[href]:not([tabindex="-1"]), button';
  function setPhoneMenu(open) {
    const button = $('menu-open');
    const main = document.querySelector('#main');
    if (!button) return;
    document.documentElement.classList.toggle('menu-open', open);
    button.setAttribute('aria-expanded', String(open));
    if (main) main.inert = open;
    if (open) {
      const first = [...$('side-menu').querySelectorAll(MENU_LINKS)].find((n) => n.getClientRects().length > 0);
      if (first) first.focus();
    } else {
      button.focus();
    }
  }
  document.addEventListener('click', (e) => {
    if (e.target.closest('#menu-open')) setPhoneMenu(!phoneOpen());
    // Using anything in the open menu (a page, Sign out) closes it; signing out does not load a new page.
    else if (phoneOpen() && e.target.closest('#side-menu a[href], #side-menu button')) setPhoneMenu(false);
  });
  document.addEventListener('keydown', (e) => {
    if (!phoneOpen()) return;
    if (e.key === 'Escape') { e.preventDefault(); setPhoneMenu(false); return; }
    if (e.key !== 'Tab') return;
    const stops = [$('menu-open'), ...$('side-menu').querySelectorAll(MENU_LINKS + ', #gs-input')].filter((n) => n.getClientRects().length > 0);
    const at = stops.indexOf(document.activeElement);
    const next = e.shiftKey ? (at <= 0 ? stops.length - 1 : at - 1) : (at === stops.length - 1 ? 0 : at + 1);
    e.preventDefault();
    stops[next].focus();
  });
  // Grown past phone width with the menu open: the page is the desktop one again.
  window.addEventListener('resize', () => {
    const button = $('menu-open');
    if (phoneOpen() && button && button.getClientRects().length === 0) setPhoneMenu(false);
  });

  // Scrolling (focus can scroll the item into view) moves the tooltip with its item.
  document.addEventListener('scroll', () => { if (tipItem) showMenuTip(tipItem); }, true);

  window.ui = ui;
})();
