---
id: admin-ui/components
title: Components
summary: The shared building blocks of the admin site, the macros and script helpers every page is made from.
keywords: [components, macros, admin UI, buttons, notices, tables, side panel, dialog, status]
settings: []
---

# Components

Every page of the admin site is built from these, so no page writes its own copy of a button, notice or table. The macros are in `services/admin/src/ui/templates/components.html`, and the icons they call are in `icons.html`. The script builders are in `services/admin/src/ui/components.js`, which exposes them as `window.ui`. A new page uses them; it does not copy their markup. Live examples are added later (plan step 7.5).

## Template macros

Each macro is called from a page template as `{% call c::name(...) %}`. The first list is for the page frame and the rest for the page itself.

- **Frame:** `skip_link`, `announcer` (the two live regions), `toast_region`, `banner_region`.
- **Page parts:** `page_header` (title, help button, lede and actions), `help` (the ? button), `button`, `icon_button`, `icon`, `tabs`, `menu`, `row_menu`.
- **Forms:** `field`, `new_secret` (a new password with its length hint), `save_bar`, `notice`.
- **Panels and boxes:** `side_panel` (docked), `dialog_box` (modal), `row_confirm`, `secret` (a show-once key).
- **Lists:** `table_head`, `status` (a symbol and a word, never colour alone), `empty`, `skeleton`.
- **Not available yet:** `na` (wraps a control that does not work yet, with the reason) and `na_tag` (a small tag with the same reason).

## Script helpers

`window.ui` has these helpers. They build the DOM with `textContent` and never assign HTML, because the admin's Trusted Types policy is `'none'`.

- **Setup:** `ready` (resolves when the limits are loaded), `constants`, `MS`, `el`.
- **Controls:** `button`, `status`, `notice`, `empty`, `menu`, `rows`, `live`, `combobox`, `highlight`.
- **Messages:** `announce`, `toast`, `banner`, `bell`, `alertIcon`, `alertPop`, `alertPopClear`.
- **Forms:** `fieldError`, `formErrors`, `lengthHint`, `trackChanges` (the save bar), `busy`.
- **Boxes and panels:** `openBox`, `closeBox`, `confirm`, `rowConfirm`, `openPanel`, `closePanel`, `panelOpen`.
- **Values:** `time`, `num`, `copy`.

Limits and intervals are never typed in a template or a script. They come from `ui.constants`, which the admin serves from `services/admin/src/constants.rs`.
