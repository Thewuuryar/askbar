---
id: admin-ui/components
title: Components
summary: The shared building blocks of the admin site, the macros and script helpers every page is made from.
keywords: [components, macros, admin UI, buttons, notices, tables, side panel, dialog, status]
settings: []
---

# Components

Every page of the admin site is built from these, so no page writes its own copy of a button, notice or table. The macros are in `services/admin/src/ui/templates/components.html`, and the icons they call are in `icons.html`. The script builders are in `services/admin/src/ui/components.js`, which exposes them as `window.ui`. A new page uses them; it does not copy their markup.

In the admin, a live example follows each macro: it is the macro itself, called from `services/admin/src/ui/templates/live.html`, so it shows what the admin uses. GitHub does not show the examples. The labels in them are placeholders.

## Template macros

Each macro is called from a page template as `{% call c::name(...) %}`. They are listed by what they are for.

### Frame

These are written once, by the page frame (`base.html`), and every page has them, this one too. They have no example of their own: the skip link shows when it has focus, and the regions are empty until a script fills them.

#### `skip_link`

The first Tab stop. It jumps to the page's `main`.

#### `announcer` live regions

Two hidden live regions: one polite and one urgent. `ui.announce` writes to them.

#### `toast_region`

Where `ui.toast` puts a short message.

#### `banner_region`

Where `ui.banner` puts a message that concerns the whole admin.

### Page parts

#### `page_header`

The title of the page (its one `h2`), a help button, a line under the title and the page's one main action.

<!-- live: page_header -->

#### `help`

The `?` button. It opens the help for a topic in the side panel; its name and tooltip are "Help:" and the topic's title. The topic must be one of the help topics in `constants.rs`.

<!-- live: help -->

#### `section_title`

The name of a settings section with its help button beside it.

<!-- live: section_title -->

#### `button`

A button. Its kind is primary, secondary, danger or submit, its size is the default, small or large, and it can have an icon.

<!-- live: button -->

#### `icon_button`

A button that holds only an icon. It needs a label, which is its name and its tooltip.

<!-- live: icon_button -->

#### `icon`

One icon from `icons.html`, by name. Icons are decorative, so the text beside them carries the meaning. These are some of them.

<!-- live: icon -->

#### `tabs`

A bar of links between the pages of one group. The labels and addresses come from the page list, so the bar matches the menu.

<!-- live: tabs -->

#### `menu`

A popup menu: a list of items opened from a button. It is closed until opened; here it is shown open.

<!-- live: menu -->

#### `row_menu`

The `...` button at the end of a row and its menu of actions. Delete goes last and is red.

<!-- live: row_menu -->

### Forms

#### `field`

A label, a control, a hint and a place for an error. An optional field says so in its label.

<!-- live: field -->

#### `number_field`

A `field` for a number, with the allowed range and its unit as the hint ("1 to 100 items"). The limits come from `constants.rs`.

<!-- live: number_field -->

#### `settings_section`

One section of a settings page: its title and hint at the left, its fields at the right. The body is the fields, in one `sec-fields` element (a `div`, or the page's `form`). The hint is template text, written as is.

<!-- live: settings_section -->

#### `new_secret`

A new password or passphrase, typed twice, with a hint about the shortest length. The length comes from the limits in `constants.rs`.

<!-- live: new_secret -->

#### `save_bar`

The bar that appears when a value on a settings page has changed, with Discard and Save. It is hidden until a change; here it is shown.

<!-- live: save_bar -->

#### `notice`

A message in the page: an error, a warning or a note, with what to do about it. The kind is also written in words, never only in colour.

<!-- live: notice -->

### Panels and boxes

#### `side_panel`

A panel docked at the right of the page, such as help. It is closed until opened; here it is shown in place.

<!-- live: side_panel -->

#### `dialog_box`

A modal dialog, for a decision that needs the whole page's attention. It is closed until opened; here it is shown open.

<!-- live: dialog_box -->

#### `row_confirm`

A question under a row before something is done to it, with Cancel and the action. It is hidden until asked; here it is shown.

<!-- live: row_confirm -->

#### `secret`

A key that is shown once, with a button to copy it. A script fills it; here it holds a placeholder.

<!-- live: secret -->

### Lists

#### `table_head`

The head of a table. A column can be right-aligned for numbers, and the last, unnamed column holds the row's actions.

<!-- live: table_head -->

#### `status`

A state as a symbol and a word, never colour alone: ok, warning, error, off, paused or pending.

<!-- live: status -->

#### `empty`

What a list shows when it has nothing, with a way to add the first item when there is one.

<!-- live: empty -->

#### `skeleton`

Grey bars shown while a list loads.

<!-- live: skeleton -->

### Not available yet

#### `na`

Wraps a control that does not work yet. It stays in its place, can be focused, and says "Not available yet" with the reason on hover and focus. It is never the `disabled` attribute.

<!-- live: na -->

#### `na_tag`

A small "Not available yet" tag with the same note.

<!-- live: na_tag -->

## Script helpers

`window.ui` has these helpers. They build the DOM with `textContent` and never assign HTML, because the admin's Trusted Types policy is `'none'`.

- **Setup:** `ready` (resolves when the limits are loaded), `constants`, `MS`, `el`.
- **Controls:** `button`, `status`, `notice`, `empty`, `menu`, `rows`, `live`, `combobox`, `highlight`.
- **Messages:** `announce`, `toast`, `banner`, `bell`, `alertIcon`, `alertPop`, `alertPopClear`.
- **Forms:** `fieldError`, `formErrors`, `lengthHint`, `trackChanges` (the save bar), `busy`.
- **Boxes and panels:** `openBox`, `closeBox`, `confirm`, `rowConfirm`, `openPanel`, `closePanel`, `panelOpen`.
- **Values:** `time`, `num`, `copy`.

Limits and intervals are never typed in a template or a script. They come from `ui.constants`, which the admin serves from `services/admin/src/constants.rs`.
