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
  // condition ends (ui.banner(key, null)). Read out assertively.
  ui.banner = (key, kind, text) => {
    const region = $('banners');
    if (!region) return null;
    const old = region.querySelector('[data-banner="' + CSS.escape(key) + '"]');
    if (!kind || !text) { if (old) old.remove(); return null; }
    if (old && old.dataset.kind === kind && old.textContent === text) return old;
    const banner = el('div', { class: 'banner banner-' + kind, 'data-banner': key, 'data-kind': kind, text });
    if (old) old.replaceWith(banner); else region.append(banner);
    ui.announce(text, { urgent: true });
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
  // waits until the name is typed (rule 31). Built like the dialog_box macro; removed when closed.
  ui.confirm = (opts) => new Promise((resolve) => {
    const id = 'confirm-' + Date.now().toString(36);
    const title = el('h2', { class: 'box-title', id: id + '-title', text: opts.title });
    const body = el('div', { class: 'box-body' });
    if (opts.body) body.append(el('p', { text: opts.body }));
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

  // ------------------------------------------------------------- lists

  // ui.rows(tbody, items, render, { key, empty }) replaces the rows: render(item) returns a <tr>, key(item) its
  // stable id (default item.id). Focus inside a row that is still there stays on the same control.
  ui.rows = (tbody, items, render, opts = {}) => {
    const key = opts.key || ((x) => x.id);
    const active = document.activeElement;
    const activeRow = active && tbody.contains(active) ? active.closest('tr[data-key]') : null;
    let focusKey = null;
    let focusIndex = -1;
    if (activeRow) {
      focusKey = activeRow.dataset.key;
      focusIndex = [...activeRow.querySelectorAll('button, a, input, select, textarea, [tabindex]')].indexOf(active);
    }
    const rows = items.map((item) => {
      const tr = render(item);
      tr.dataset.key = String(key(item));
      return tr;
    });
    if (rows.length === 0 && opts.empty) {
      const cols = tbody.closest('table') ? tbody.closest('table').querySelectorAll('thead th').length : 1;
      rows.push(el('tr', { class: 'empty-row' }, el('td', { colspan: String(cols || 1) }, opts.empty)));
    }
    tbody.replaceChildren(...rows);
    if (focusKey !== null) {
      const again = rows.find((r) => r.dataset.key === focusKey);
      const controls = again ? [...again.querySelectorAll('button, a, input, select, textarea, [tabindex]')] : [];
      if (controls[focusIndex]) controls[focusIndex].focus();
    }
    return rows;
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

  window.ui = ui;
})();
