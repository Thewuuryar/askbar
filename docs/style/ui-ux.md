# UI and UX

How the popup and the admin site look and behave. Items marked **(proposed)** are not decided. The look-and-feel rules below are the target; the CSS on `main` still differs (two palettes, mixed radii) and moves to them in the UI change that follows.

## Look and feel

**Intent: a quiet card; the input bar is the only loud thing.** Judge every UI change against that sentence. If a change adds something louder than the input, remove it or justify it in the PR.

### Tokens and scales

- **One token set for both UIs.** Colours, sizes, spacing, radii and weights are CSS variables defined once, in `clients/desktop/ui/tokens.css`. The admin site uses a byte-identical copy (`services/admin/src/ui/tokens.css`). `scripts/check-ui-tokens.sh` fails if they differ or if a text colour is under 4.5:1 in either theme (one place per fact). Names: `--bg`, `--card`, `--fg`, `--muted`, `--line`, `--accent`, `--danger`, `--good`. No rule uses a fallback colour. Neutrals are the popup's. The accent is the popup's blue, darkened slightly (`#3a5bd0`) so it passes 4.5:1 as text on both backgrounds; muted text and the success colour were darkened for the same reason.
- **Text: four sizes.** `0.75rem` (hints, meta), `0.875rem` (secondary), `0.9375rem` (body), `1.3rem` (input bar, headings). Two weights: 400 and 600.
- **Spacing: 4, 8, 12, 16, 24, 32 px.** No other value.
- **Radii: two.** 8 px for controls and 16 px for the popup card. No pill shapes and no 4, 6 or 12.
- **Rules, not boxes.** Separate things with whitespace and one 1 px `--line` hairline. No card inside a card, no border on a border. Only the popup card has a shadow.
- **No hard-coded colour** in a rule or in script; the `rgba(...)` values on `main` become tokens.

### Themes and contrast

- Themes are `auto`, `light` and `dark`, both built from the same tokens: each token has one light and one dark value, set in one place. `light-dark()` may replace the duplicate blocks once WebKitGTK and the macOS and Windows webviews all support it (to verify).
- Text has at least 4.5:1 contrast against its background in both themes; borders, focus rings and icons that carry meaning have at least 3:1. Never use colour alone to carry meaning.

### Accessibility

- Every action is reachable by keyboard, with a visible `:focus-visible` ring.
- Type sizes are in `rem`, so text follows the OS or browser font size. Layouts must not break at 200 %.
- Honour `prefers-reduced-motion`.

### Motion

None, or one transition of 150 ms or less (opacity or colour). Under `prefers-reduced-motion: reduce` there is none.

### Patterns for states

- **Error:** one sentence under the input. Never a modal.
- **Notice:** a dismissible bar. One at a time.
- **Loading:** one indicator, in the same place.
- **Empty:** only the bar.
- **Trust text stays quiet.** The AI warning ([ADR 0012](../adr/0012-consent-notices-and-deletion.md)) is a muted line (`--muted`, smallest size) above the chat box. The privacy notice shows once at pairing and again only when the relevant settings change.

## The popup (`clients/desktop/ui/`, Tauri)

- **A bar, not an app.** One hotkey shows and hides it. Esc stops a streaming reply; otherwise it hides. Default hotkey `Ctrl+Shift+Space`.
- **Keyboard first.** Every action has a key (README, "Using it"). Ctrl is used on macOS too, because Cmd+H and Cmd+N belong to the system.
- **Thin client.** The window has no filesystem or network permission (`clients/desktop/src-tauri/capabilities/default.json`). Only the Rust side talks to the server. Provider keys never reach the client.
- **No offline mode.** If the server cannot be reached the window says "server offline". It does not queue work silently or show stale answers as current.
- **Say it in words, keep what was typed.** A refusal (kill switch, token limit, no audit key, `audit_backlog`, queue full) is shown as a sentence and the text the person typed stays in the box. A cut-off reply keeps its text and offers **Try again**.
- **Branch on codes.** The app decides what to do from the `error` code, never from the wording. The text shown comes from the problem's `detail`.
- **Newest first or oldest first.** The input bar stays at the top either way. Newest first puts the latest turn just under it; a turn (a question, its reply, its error line) keeps its own order, and only the turns are reversed. The admin sets the default (`newest_first`, off), and `client.yaml` can override it.
- **Copy a whole conversation** from the history panel: the copy icon before Delete copies it as Markdown, with who wrote each message, when, the model and provider of each reply, and why a question failed. It does not open the conversation.
- **Destructive actions take two clicks.** Delete asks "Delete" then "Delete?".
- **Pick a model from the approved list.** Clients choose from the models the admin approved, one of which is the default. The footer shows the model in use. There is no provider fallback in v1, so a failed provider is an error sentence, not a different answer ([ADR 0014](../adr/0014-model-gateway-and-providers.md)).
- **Notices are on by default and the admin can edit or turn them off** ([ADR 0012](../adr/0012-consent-notices-and-deletion.md)). The AI warning ("AI can make mistakes. Check important information.") sits above the chat box. The privacy notice shows at pairing and when the relevant settings change.
- **Live only where it started.** The client that started a chat streams the reply. Other clients show a processing message with a short description until the chat completes ([ADR 0008](../adr/0008-users-clients-and-client-access.md)).
- **No build step.** `ui/` is plain HTML, CSS and JavaScript. Adding a framework or bundler needs an ADR.
- **Markdown links and images.** Remote images are blocked. A link shows its destination before it opens ([ADR 0016](../adr/0016-risk-register.md); a ship criterion).
- **Markdown is escaped first.** `clients/desktop/ui/markdown.js` HTML-escapes everything, then adds only the markup on its allowlist, so model output cannot inject HTML ([ADR 0016](../adr/0016-risk-register.md)). That is the only reason `renderMarkdown` output may be assigned to `innerHTML`. Keep `tests/markdown.test.js` covering it.
- **A failed question stays marked.** A question whose turn failed keeps its error line, live and after reopening: after the part of a reply that was kept, or right after the question, which then has a `--danger-soft` tint. The line says why in words and offers **Try again** (the same text asked again, below) and **Edit** (the text back in the box), for every failure kind. The failed question is not removed.
- **Every message has a meta row:** its date and time, and on a reply a small chip whose tooltip (`title`, and the same `aria-label`) names the model and provider that wrote it, then the copy icon. The row shows on hover or when one of its controls has keyboard focus, and its height is kept at all times, so nothing moves when it appears. A reply stored before servers recorded its provider shows no chip.
- **Colours and sizes come from the tokens** in "Look and feel". Do not hard-code a colour, size or radius in a rule or in script.
- **The footer is always shown and holds:** the model picker (or its label), a **Source** link, and the five shortcut hints. Source is a link like any other, so it asks first and shows its whole address before it opens: it points to the project's source, the AGPL notice the admin site also carries. The footer has no connection status; a problem with the server ("server offline", a rejected token) shows in the status banner, which sits outside the footer.
- **Tests.** Behaviour changes carry a test in `clients/desktop/tests/ui.test.js` (Playwright against `clients/desktop/tests/mock-tauri.js`).

## The admin site (`services/admin/src/ui/`)

The admin is server-rendered pages with three plain scripts and no build step. Its components and page layouts are documented in the reference, with a live example of each in the admin: [Components](../reference/admin-ui/components.md) and [Page layouts](../reference/admin-ui/page-layouts.md). This section holds the rules; it does not repeat the markup.

### Look and behaviour

- **One primary action per view.** Buttons are secondary by default (outline, no fill). The one primary action is marked explicitly and is the only one filled with `--accent`. A destructive action uses `--danger`, never the primary style.
- **Navigation.** The side menu links every page, grouped under Audit and Settings. A group's heading links to its first page (Settings opens General); the current page has `aria-current="page"`, and its group heading is bold. The menu collapses to icons (`--rail-w-collapsed`); the choice is remembered per browser, and names show on hover and on keyboard focus. On a phone the same menu opens full screen from a Menu button in a top bar.
- **Status bar.** Every page has one status bar: the breadcrumb, global search (`/` focuses it), the budget, the kill switch, the seal toggle and the alerts bell. Search covers the pages and settings, and the reference comes last under Reference. The side menu is fixed to the window and the status bar and the banners stay pinned at the top while the page scrolls; the search box sits at the centre of the bar, whatever the breadcrumb says (below 1000 px wide it shares the left side instead).
- **Page header.** Every page has a title, a `?` help button, one line of description and its actions on the right. The title sits below the status bar.
- **Sections are separated by hairlines, not boxes.** A card is used only on sign-in and first-run setup. No card inside a card, and no border on a border.
- **Desktop and phone.** The site works at phone width and desktop width: one column and a full-width menu at small widths, a side navigation with a content column at large ones. Every page has the same content column (the same left edge for the title and the first block), with the scroll bar room always kept and `--content-bottom-room` left under the last block. A table fits its column, so its box shows no scroll bar; one that cannot fit (the nowrap Audit tables, any table on a narrow window) scrolls inside its own box; the page never scrolls sideways. Browser tests run at both widths.
- **Tap targets** are at least 24 px on desktop and 44 px on a phone, and buttons are 28, 32 or 40 px high (`--btn-h-s`, `--btn-h`, `--btn-h-l`), 44 px on a phone.
- **Save and apply.** A save button says "Save". "Save and apply" is used only where the save takes effect at once (Network). A change shows a save bar, "1 unsaved change: …", with Discard and Save. A failed save shows a summary at the top of the form and moves focus to the first error.
- **Forms.** The label is above the field. A field's hint is one line under it (format, range, limit): never placeholder text, and never in brackets in the label. Mark "(optional)" only when most fields are required. A new password or passphrase is two fields stacked full width, with the length rule under the first as a live count, and "The two passwords do not match." (or passphrases) under the second as soon as it has text that differs from the first. Paste is always allowed.
- **Help and text.** Every page title and every section heading has a `?` button after it (the exceptions: the reference's own pages, which are the help, headings that only label a list of links, headings inside a side panel or box, and the sign-in and setup cards), and a section has at most one line of help. Explanations live in the reference. Text stays on screen only when it prevents a mistake or is needed to finish a task: lock-out warnings, "cannot be recovered", required steps.
- **Side panel.** One panel at a time, docked on the right under the status bar and 440 px wide (`--panel-w`), used for adding and editing, for help, and for the selected Audit row. The page narrows and stays usable; nothing is dimmed. Esc and × close it, unsaved changes ask first, and it has its own address so Back closes it.
- **Boxes are modal** and used only for confirmations and one-off secrets (a pairing key, a new API key).
- **Risky actions.** A risky action on one row (restarting a service) opens that row to confirm: Cancel and the action at its bottom right, and the row's own button hides until it closes. Deleting asks for no confirmation when Save or Undo covers it, asks in a box when it cannot be undone, and asks the person to type the name when it destroys data or affects everyone. Anything that interrupts something says what it will interrupt before it asks (the Services page does this).
- **Toasts** are for success and Undo only. Errors are inline, next to the fix.
- **Lists.** One visible row action and a ⋯ menu; Delete is last, in red. A list refreshes every `LIVE_EVERY_SECS` while the tab is visible: new rows are highlighted and announced, scroll, focus and typing are kept, and the admin's own changes show at once. While the tab is hidden the bell and the tab title refresh every `HIDDEN_EVERY_SECS`. A failed check backs off and shows "Reconnecting…".
- **Numbers and times.** Relative times, with the full date on hover. Numbers are right-aligned with their units.
- **Statuses** use a fixed set of words, always with a symbol: never colour alone.
- **Audit pages** show a table with a few numbers above it (replies, errors, tokens, p50 and p95 time). Use `--danger` only for errors. A chart needs a one-line text summary, and nothing animates under reduced motion.
- **Greyed-out features.** A feature the server cannot support yet is drawn with `na()`: `aria-disabled="true"`, never `disabled`. Tab reaches it, Enter does nothing, and a "Not available yet: …" note shows on hover and on focus. A greyed item has no digit in its text.
- **Secrets.** A secret (API key, pairing key, token) is shown once, at creation, and the screen says so.
- **Server down.** A change made while the server is down is refused with a sentence and is not half applied. The admin does not queue it ([ADR 0028](../adr/0028-service-boundaries-and-config-delivery.md)); it pushes the saved state when the server returns.
- **Sealed content** ([ADR 0011](../adr/0011-sealed-content-access.md)). While the session is open the status bar shows the time left and a **Seal** button that ends it at once. Opening and timeout changes raise an alert, and the screen says so before it asks for the passphrase.

### Security (admin)

- **Text in, never HTML.** Text from the server goes in through `textContent`, never `innerHTML`. The CSP forbids inline script and style, and it requires Trusted Types with no policy (`trusted-types 'none'`). Do not loosen it.
- **Help and reference text** is rendered when the admin is built, never fetched or injected at run time (see [Help](#help)).

### Alerts and history

- **The bell** badge counts critical plus warning. Its menu groups Critical, Warning and Info, and a new critical alert also pops up.
- **Banners are for the whole admin only.** A banner is shown for a condition that affects the whole admin. It cannot be dismissed, it clears itself when the condition ends, and it shows the current state only. Which alerts get a banner is set in Notifications; critical ones are always shown.
- **History goes in the bell and on the Alerts page, never in a banner.** A banner that carries history would read as a current problem after it has passed.

### Accessibility (admin)

- **Keyboard.** Everything works by keyboard. The focus ring is never hidden behind a sticky bar, and "Skip to content" is the first Tab stop.
- **Names.** Every control has a name, and every tooltip also shows on keyboard focus.
- **Announcements.** One announcer (`ui.announce`): updates are read out without moving focus. Polite, except critical alerts and banners, which are urgent.
- **Contrast.** Text is at least 4.5:1 and icons and field borders at least 3:1, in both themes. `scripts/check-ui-tokens.sh` checks the token pairs.

### Why some colours and tooltips are the way they are

- **The current page's name is `--fg`, not the accent.** In the side menu the current item has an accent tint (`--accent-soft`) and an accent icon. The accent text on that tint is 4.48:1 in the light theme, under the 4.5:1 a name needs; `--fg` on it is about 13:1. The icon is not text, so the accent is kept there (3:1 is enough). The side menu's background is `--bg`.
- **The menu tooltip is one fixed element, `#menu-tip`.** The collapsed side menu and the menu items show their names in it. It sits outside the menu, because the menu scrolls and a tooltip inside it would be clipped. It is placed beside the item, repeats the item's accessible name (so it is `aria-hidden`), and Esc hides it. See the menu code in `services/admin/src/ui/components.js`.

### Help

Each help button (`help()`) opens the docked `#help-panel` with the text of its topic. The text was rendered when the admin was built, from `docs/reference` (`services/admin/build.rs` and `services/admin/src/reference.rs`), and each page carries it in a `<template data-help-topic>`, so nothing is fetched and no HTML is injected at run time. The topics are the `HELP_TOPICS` list in `constants.rs`: a page's topic is its reference page's id (the text before its first heading) and each section's is `<page id>#<heading>`. A new page or section gets its text in `docs/reference` first, then its topic, then the `help()` call; a test fails for a page with a title or section heading and no button. "Open full page" goes to `/admin/reference/`, which renders the same reference; see [the reference index](../reference/index.md). To change what a help topic says, edit its page in `docs/reference`.

### Admin pages

Each page is its own URL, `/admin/<slug>/`, pre-rendered HTML with no client-side routing (plan step 1.16). The pages, their groups and their icons are the one list `PAGES` in [`pages.rs`](../../services/admin/src/pages.rs). The side menu, the routes and the route-table test all come from it, and `PAGE_SLUGS` in `constants.rs` lists the same slugs. The menu has Home and Alerts at the top, then the Audit group (Model calls, Admin actions, Sealed access, Routing), the Settings group (General, Providers, People & clients, Services, Network, Security, Backup, Notifications) and Reference.

- **One frame:** `services/admin/src/ui/templates/base.html`. It holds the head, the side menu, the status bar, the banners and the page header. The header takes its one-line description from `PAGES`. A page's template fills the `content` block, and may fill `scripts` for a page-only script. The sign-in and first-run screens are the card layout, with no menu.
- **A new page** is one `PAGES` entry, one `page!` line in `pages.rs`, and one `ui/templates/<slug>.html` that extends `base.html`. If the page loads data, add its loader to the `tabs` table in `app.js`. Add its slug to `PAGE_SLUGS` in `constants.rs`.
- **Templates hold static text, the page list and constants only.** Askama escapes them. Everything that comes from the API goes in through `textContent` in the scripts.
- **Components.** Pages are built from the macros in `ui/templates/components.html`, and icons from `icons.html`. The macros' header comment is the list of them. `blocks.html` holds the two building blocks `error_line(id)` and `hint(text)`. Script builders are in `ui/components.js`, which exposes them as `window.ui`. A page never writes its own copy of a component's markup.
- **Scripts.** `components.js` (the builders and the frame's behaviour), `app.js` (the API, the bell, the status bar and the loaders), `audit.js` (Sealed access and Routing) and `audit_calls.js` (Model calls). A script wires each element only if it is on the page (`on(id, event, fn)`), and runs only the current page's loader.
- **Addresses.** State a person may want to link to goes in the query string and is kept current with `history.replaceState`: the selected trace on Model calls, and the open panel (`?panel=`). The old `?view=trace&id=` address still opens the trace.
- **Caching.** Static files are sent with an `ETag` and `no-cache`, so a page change costs one page plus 304s. Pages and the API are `no-store`.

### Single source of truth

- **Behaviour values and limits** live in `services/admin/src/constants.rs`: limits, intervals, the page list, the help topics, the links. They are served as `/admin/constants.json` and read in the scripts as `K` (`app.js`) or `ui.constants` (elsewhere), and in templates as `{{ crate::constants::NAME }}`. No number, limit or URL is typed into a script or a template.
- **Visual values** live in `tokens.css`: colours, sizes, spacing, radii, the panel width, the status bar and menu widths. The admin copy (`services/admin/src/ui/tokens.css`) is byte-identical to the popup's, and only gains variables.

### Drift checks

These fail the build when the rules above are broken.

- **Components are not hand-written.** `services/admin/tests/drift.rs` (`no_template_hand_writes_markup_a_component_covers`) fails when a template writes out markup that a component covers. The check is a list of the markup each component owns (`COVERED`). To allow one case, add a row to `MARKUP_EXEMPT` with the file, the marker, the context and a reason. An exemption that matches nothing fails too.
- **No literal for a constant.** `no_script_or_template_types_in_a_number_the_constants_define` fails when a script or template types a number that `constants.json` defines. Time-unit maths (`60`, `1000`, `3600`, `24`) and everyday digits are allowed. To allow another, add a row to `LITERAL_EXEMPT` with the file, the number, the context and a reason; read the value from `K` or `ui.constants` instead wherever you can.
- **No stale page address.** `every_page_address_in_the_scripts_and_templates_names_a_page` fails on an `/admin/<slug>/` for a slug that is not in `PAGE_SLUGS`. The reference (`/admin/reference/...`) and the admin API (`/api/admin/v1/...`) are not checked. There is no exemption list: add the page, or fix the address.
- **Greyed items have no digit.** The end-to-end step "greyed items: no "Not available yet" item shows a digit, on any page" in `tests/e2e/admin.test.js` visits every page and fails on a greyed item with a digit. To allow one, add its label and a reason to the step's `EXEMPT` list.

### Tests

The browser suite, `tests/e2e/admin.test.js`, runs in a real browser against the real server and admin, at 1280 and 390 pixels. Every page is checked with axe, target sizes, the CSP and Trusted Types, and the script errors.

## Other clients

- **iPhone app (ADR 0008: preferred, can be deferred past v1).** It is a native Swift app. It uses the same API, error codes, glossary names and rules as the desktop app, including the tokens, the footer (connection status and model picker), the notices, and: server offline in words, typed text kept, no keys on the phone. It follows Apple's accessibility settings, including Dynamic Type.
