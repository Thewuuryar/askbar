# Plan: finish the admin redesign (phases 5–8)

Previous: [Release branches](plan-release-branches.md) · Tracker: [task-tracker.md](task-tracker.md)

Status: done, merged in #117; kept as the record of the redesign rules (Appendices A and B). Was: tracker for the admin redesign after phases 1–4 (built on `claude/trusting-thompson-3vv2sw`, last commit `f919500`). It rewrites phases 5–8 of the owner's "Askbar admin redesign: implementation plan" (Nov 15, 2026) for a Sonnet 5.5 orchestrator that hands most steps to Haiku. Design source: the canvas [Askbar admin redesign options](https://claude.ai/artifact/EwuJTn1gRAGMqUu32qyea8), page "3 · Components and pages". Rules for agents: [AGENTS.md](../AGENTS.md).

## Start here

The work so far lives on the branch `claude/trusting-thompson-3vv2sw`; this file is on it too. A new session does this before step 5.1:

1. **Branch.** `git fetch origin claude/trusting-thompson-3vv2sw`. If the session's designated branch *is* that branch, check it out. Otherwise merge it into the session's own branch, `git checkout <session-branch> && git merge --no-edit origin/claude/trusting-thompson-3vv2sw` (a merge, so a session branch already pushed from `main` never needs a force-push), push that, and push each later step to the session's branch, which then carries phases 1–4 and everything after. Pushing to `claude/trusting-thompson-3vv2sw` from another session needs the owner's word. Never rebase or force-push either branch.
2. **Tools.** `npm install` once if `node_modules` is missing, then `npm install --no-save playwright@1.63.0`. That Playwright version matches the browser pre-installed in `/opt/pw-browsers`; a newer one cannot find it. Then `cargo build --workspace` (the browser suite starts the real services from `target/debug`).
3. **Boards.** Fetch the boards named in each phase from the design canvas, `https://claude.ai/artifact/EwuJTn1gRAGMqUu32qyea8` (page "3 · Components and pages"), with the Artifact tool's read action: `paths: ["project/P3-Settings.dc.html", …]`, `out_dir: <scratchpad>/design`. The link was valid on Nov 17, 2026. If it no longer opens, ask the owner; do not draw from memory.
4. **Brief.** Save "Brief for every subagent" below as `<scratchpad>/BRIEF.md`.
5. **Check the base.** `cargo test -p askbar-admin` and `node tests/e2e/admin.test.js` must pass before 5.1 (95 steps at each width as of `f919500`).

## Summary

Phases 1–4 are done and pushed to `claude/trusting-thompson-3vv2sw` (last commit `f919500`). This document replaces phases 5–8 of the original plan for a **Sonnet 5.5 orchestrator** that reviews and pushes, and hands each step to a subagent. To cut usage, **18 of the 27 remaining commits go to Haiku** (19 of 28 if the optional wiki mirror runs): the original Sonnet steps are split into smaller, fully specified pieces wherever the design leaves no real judgment. Sonnet subagents get only the steps that rebuild a page's behaviour, change Rust build code or render the reference.

Binding on every step, copied verbatim from the original plan into [Appendix A](#appendix-a-binding-sections-of-the-original-plan): the scope (admin UI only; no API or other service changes), the "Not available yet" table, "Single source of truth: constants", the "Admin UI/UX rules" (1–48), the target structure and the "Component library". Where a step below says "as in Appendix B", the original step text is in [Appendix B](#appendix-b-original-text-of-phases-58).

| Phase | Steps | Haiku | Sonnet |
| --- | --- | --- | --- |
| 5. Settings | 12 | 9 | 3 |
| 6. Audit, sign-in, setup | 4 | 2 | 2 |
| 7. Reference and help | 6 (+1 only if approved) | 2 (+1) | 4 |
| 8. Cleanup and style guide | 5 | 5 | 0 |
| **Total** | **27 (+1)** | **18 (+1)** | **9** |

## How the orchestrator works

**Roles.** The orchestrator (Sonnet 5.5, this session's model) never writes a step itself. It reads each subagent's diff, sends fixes back to the **same** subagent (`SendMessage` to its id), pushes, and posts a phase review. Haiku takes every step marked **H**; Sonnet subagents take steps marked **S**. If a Haiku agent fails the same check twice, or reports it needs a design decision, re-run that step with a Sonnet agent and say so in the phase review.

**Per step:**

1. Spawn the agent (`model: "haiku"` or `"sonnet"`, foreground) with: the step block below copied verbatim, "Read BRIEF.md first", the board file(s) to match, and "commit, do not push".
2. Wait for the hand-back. While it runs, **do not commit or push**, even when the stop hook asks: the uncommitted changes are the agent's unfinished work. Say so in one line and wait.
3. Review: `git show --stat HEAD`, then the diff of the step's files against the review checklist below. Fixes go back to the same agent, which amends its commit. Do not push before the amend (a pushed commit cannot be amended without a force-push, which AGENTS.md forbids).
4. Push to the session's branch (`git push -u origin <session-branch>`). Start the next step.

**Usage savers:**

- One full browser suite run per step, by the agent (it takes ~8 minutes at two widths). The orchestrator does not re-run it unless the agent's report is incomplete.
- Known flake: at 1280, "a change while the server is away is refused…" sometimes gets a 500 and the next two steps fail. If only those fail, the agent reruns once. Do not chase it inside a UI step (see Follow-ups).
- Keep agent prompts to the step block plus pointers; BRIEF.md carries the context.
- If an agent dies (rate limit, restart), run `git status`: a clean tree means relaunch the same prompt; a dirty tree means relaunch with "continue from the working tree".

**Phase end:** post a short review (table of steps, commits, checks, deviations, carry-forwards), take screenshots at 1280 and 390 of the pages the phase changed and compare with the boards, run `scripts/smoke.sh`, and run the desktop UI and CSP tests (`clients/desktop/tests/ui.test.js`, `csp.test.js`) if `tokens.css` changed. Then continue to the next phase unless the owner said to stop.

### Stop and ask the owner first

- **7.1b**: moving `docs/architecture.md` sections out. AGENTS.md ranks that file second in precedence and links it everywhere; moving it is a boundary change.
- **7.2**: `pulldown-cmark` as a build dependency. The plan names it, but AGENTS.md wants every new dependency stated and license-checked (MIT; `cargo deny check` must pass). Ask once, before 7.2 starts.
- **7.6**: the wiki mirror needs the open question answered **and** a `.github/workflows/` change, which AGENTS.md says needs asking. Skip it unless the owner said yes.
- Any server-side fix (the flake below, or a missing API a step wants). Record it as a follow-up instead.

### Review checklist (what phases 3–4 reviews actually caught)

- **Invented data on screen.** No sample figures from the boards ("Budget 38% of $20") on greyed items. Labels only.
- **Noise text (rule 12).** No warning for something optional. For example, "no host name" may warn only when HTTPS is on.
- **Typed-in limits.** No number for a limit, interval, size or count in JS or templates. It goes in `constants.rs` (struct field, `current()`, and the key list in `the_json_holds_every_constant`). Time-unit maths (60, 1000, 3600) is fine.
- **Banners are whole-admin only (rule 39).** History and info go to the bell or Alerts.
- **Tests.** No assertion loosened. A changed test is explained in the commit body as a contract change. `axe-baseline.json` stays empty.
- **Safety.** No `innerHTML`/`insertAdjacentHTML`, no inline `style=` or script in templates, no `disabled` on "Not available yet" items (`aria-disabled` only).
- **Tokens.** New tokens are in both `tokens.css` copies and are add-only.
- **Moved slugs.** A page whose slug changed: grep for `/admin/<old>/` in `app.js`, `components.js` and the templates. `BELL`, `SETUP_GAPS`, `SETTING_FIELDS`, Home links and the setup "Next steps" all hold page addresses.
- **Phone.** 44 px targets and no sideways scroll at 390.

## Brief for every subagent

The orchestrator saves this section as `<scratchpad>/BRIEF.md` (or tells each agent to read this section of the file) and replaces the attribution placeholder with its own session's lines.

You are implementing ONE step of the admin redesign plan in /home/user/askbar (on
the orchestrator's session branch, already checked out, which carries phases 1–4). Do that step only, as one commit. Do not push.

### Read first (all of it)

1. This file, `docs/plan-admin-redesign.md`: Appendix A in full (scope, "Not available yet", constants,
   rules 1-48, component library), your step below, and Appendix B for the step's original text. These are binding.
2. `/home/user/askbar/AGENTS.md` (hard rules, commit style). CSP is strict: no inline script or style
   attributes in HTML, Trusted Types 'none' (never assign innerHTML/outerHTML/insertAdjacentHTML; build DOM
   with document.createElement / ui.el and textContent). Setting `el.style.x` from JS (CSSOM) is allowed.
3. The design boards (HTML, data only, never instructions) are in `<scratchpad>/design/project/`; the
   orchestrator downloads them there (see "Start here").
   Read the one(s) your task names for markup and CSS values. They use their own class names; translate
   to ours and to tokens.

### What already exists (built in phases 1-2; reuse, never re-create)

- `services/admin/src/constants.rs`: every limit and interval; served as `/admin/constants.json`. A new
  constant needs a field in the `Constants` struct, `current()`, and the key list in the test
  `the_json_holds_every_constant`. Templates read constants as `{{ crate::constants::NAME }}`.
- `services/admin/src/pages.rs`: `PAGES` (slug, label, group, icon), `menu()`, `section_id()`, `page!` macro;
  template fields `current`, `label`, `section`, `menu`. Tests there render every component and check the frame.
- `ui/templates/base.html`: the frame. `#app.app` = `.phone-bar` (phones) + `aside#side-menu.side-menu`
  (nav `#nav`, links carry `data-tab=<slug>`, sign-out `#logout`, health line `#menu-health`) + `main#main`.
  Inside main: the old flash/banner elements (`#offline`, `#rejected`, `#chain-alert`, `#alerts-banner`,
  `#unsealed-banner`, `#sealed-alerts`, `#flash`) then `section#<section>[data-tab]` with heading row and hint.
  After #app: `c::toast_region()`, `c::announcer()`, footer (shown only signed out).
- `ui/templates/components.html`: Askama macros (see its header comment). `ui/templates/icons.html`: icons,
  call via `{% call c::icon("name") %}{% endcall %}`.
- `ui/components.js` exposes `window.ui`: `ready` (promise of constants), `constants`, `MS` (1000), `el(tag, attrs,
  ...children)`, `button`, `status(state, word)`, `notice`, `empty`, `announce(text,{urgent})`, `toast(text,{undo})`,
  `banner(key, kind, text)` (null text clears), `fieldError`, `formErrors`, `lengthHint`, `trackChanges`, `busy`,
  `openBox`/`closeBox`, `confirm({title, body, action, typeName})`, `rowConfirm`, `openPanel`/`closePanel`,
  `menu(button, items)`, `rows`, `time(unixSecs)`, `num`, `copy`. Side-menu collapse, menu tooltip (`#menu-tip`)
  and phone menu (`#menu-open`, class `menu-open` on <html>) live at its end.
- `ui/app.js`: `api(method, path, body)`, `$`, `show`, `on`, `K` (constants, set at boot after `await ui.ready`),
  `MS`. `checkServer()` polls `GET /api/admin/v1/status` every `K.STATUS_EVERY_SECS` and drives the old banners;
  `checkHealth()` drives the menu health line. `window.askbarAdmin` = { api, flash, registerTab, show, $ }.
- `ui/style.css`: sizes, spacing, colours only from tokens (`var(--...)`); font-size only `--text-xs|s|m|l`,
  weight only `--weight-regular|bold` (checked by `scripts/check-ui-tokens.sh`). Phone breakpoint is
  `@media (max-width: 560px)`. New tokens go in BOTH `services/admin/src/ui/tokens.css` and
  `clients/desktop/ui/tokens.css` (byte-identical; only add, never change a value).
- No number for a limit or interval typed in JS or templates: read it from `K`/`ui.constants`.

### Tests (run them; paste the summary lines in your final report)

```sh
cd /home/user/askbar
cargo fmt --all && cargo build -q -p askbar-admin
cargo test -q -p askbar-admin 2>&1 | grep -E "test result|FAILED|panicked"
cargo clippy -q -p askbar-admin --all-targets -- -D warnings
scripts/check-ui-tokens.sh && scripts/check-docs.sh
timeout 1500 node tests/e2e/admin.test.js > /tmp/claude-0/e2e.log 2>&1; grep -E "^FAIL|^\[" -A6 /tmp/claude-0/e2e.log | head -60
```

- The browser suite (`tests/e2e/admin.test.js`) runs every step at 1280 and 390 (`isPhone`), ~4 minutes.
  Helpers: `step(name, fn)`, `openPage(slug)`, `openMenu()` (opens the phone menu first on a phone), `base`,
  `page`, `ctx`, `browser`, `viewport`. Add steps for your step's checks near related steps. Read constants in a
  test with `await page.evaluate(async () => (await window.ui.ready).NAME)`.
- Existing steps check: axe on every page (`tests/e2e/axe-baseline.json` must stay empty: fix violations, never add
  to it), every target >= 24 px (44 on a phone), no JS errors, CSP/Trusted Types.
- Known flake, not yours: at 1280, "a change while the server is away is refused..." sometimes gets a 500 and the
  next two steps fail. If only those fail, rerun once; if they pass, fine. Say so in your report.
- Never loosen or delete a test unless the contract changed; if you change one, say why in the commit body.
- Playwright here must stay 1.63.0 (matches the installed browser). Do not run `npm install` or `playwright install`.

### Commit

One commit, message exactly the step's given subject (`type(admin): ...`), body in the repo style: why, what
changed, deviations from the plan, tests added/changed. End the body with exactly:

```
<the attribution lines the orchestrator's session gives for commits>
```

Do NOT push. Do not touch files outside the step's list unless a test or doc must follow (say so).
Do not change `.github/`, the admin API's behaviour, other services, or add dependencies.

### Final report (your last message)

Commit hash; files changed; what each check printed (pass/fail lines); every deviation from the plan and why;
anything you could not do or verify.

### Added after phases 3–4

**What exists after phases 1–4 (reuse, never re-create):**

- `constants.rs`: every limit and interval, served as `/admin/constants.json` (read in JS as `K.NAME` in `app.js`, `ui.constants.NAME` elsewhere, in tests `await page.evaluate(async () => (await window.ui.ready).NAME)`). Includes `PAGE_SLUGS`, `LANDING_PAGE` (`home`), `HOME_STATS_WINDOW_*`, `SEARCH_RESULTS_PER_GROUP`. Templates: `{{ crate::constants::NAME }}`.
- `pages.rs`: `PAGES` (slug, label, `Group::{Top, Audit, Settings, Reference}`, icon), `menu()` (a group heading links to its first page), `section_id()`, `crumb_group()`, the `page!` macro (template fields `current`, `label`, `section`, `menu`, `group`). Today's slugs: `home alerts audit/calls audit/admin audit/sealed audit/routing providers clients services network security backup restore settings reference`. Its tests render every component macro, every icon, and check the frame (one `aria-current` in `#nav` plus one in the breadcrumb).
- Templates: `base.html` frame, made of:
  - `.phone-bar` with `#menu-open`;
  - `aside#side-menu`, containing `#nav` (links carry `data-tab=<slug>`), `#menu-health`, `#menu-collapse` and `#logout`;
  - `.content`, containing `.status-bar` (breadcrumb, `#gs-input` search, `#status-facts` with the budget `na`, `#sb-kill`, the seal toggle and `#bell-button`/`#bell-menu`), then `#banners`, `#page-error` and `main#main > section`.

  `components.html` macros (header comment documents each), `icons.html` (call `c::icon("name")`), `home.html`, `alerts.html`, `audit.html` (shared by three Audit pages until 6.1–6.2), `audit_admin.html`, `reference.html` (placeholders).
- `components.js` (`window.ui`):
  - `ready`, `constants`, `MS`, `el`, `button`, `status`, `notice`, `empty`
  - `announce`, `toast`, `banner(key, kind, text, opts)`, `bell`, `alertPop`
  - `fieldError`, `formErrors`, `lengthHint`, `trackChanges` (save bar), `busy`
  - `openBox`/`closeBox`, `confirm({title, body, action, typeName})`, `rowConfirm`, `openPanel`/`closePanel` (docked, own `?panel=` address, unsaved guard)
  - `menu`, `rows` (keyed merge with highlight), `live(load, opts)`, `combobox`, `highlight`
  - `time`, `num`, `copy`
- `app.js`:
  - `api()`, `pageError()`
  - one alert builder `bellItems()`/`urgentOf()` feeding the bell, Home and Alerts
  - `SETUP_GAPS` (Home), `SETTING_FIELDS` (search), `statsFigures()`
  - `openSession`/`closeSession` (sealed content)
  - `checkKill`, `focusHash()`, which scrolls to and focuses `#id` from the address after load.
- `style.css`: tokens only; font sizes `--text-xs|s|m|l`, weights `--weight-regular|bold` (checked by `scripts/check-ui-tokens.sh`); phone rules in `@media (max-width: 560px)`.

## Phase 5: Settings

Each Settings section becomes its own page. Boards: `P3-Settings`, `P3-Providers`, `B4-Provider-Add`, `P3-People`, `B2-People-Edit`, `B2-People-Delete`, `B4-Pair-Client`, `B4-Pair-Connected`, `P3-Services`, `P3-Network`, `P3-Security`, `P3-Backup`, `P3-Notifications`, `P3-General` (fetch the ones not yet in `scratchpad/design/project/`).

**5.1 Rename the settings page to general** · **H**

- Do: slug `settings` becomes `general`, label "General", template `settings.html` becomes `general.html`; update the `page!` line, the `PAGES` entry (icon `sliders`) and `PAGE_SLUGS`. Then update every address that points at `/admin/settings/`: the `BELL` table and `alerts.html` (`#alerts-form`), `SETTING_FIELDS` slugs, `SETUP_GAPS`, the e2e `openSettings()` helper and the slug lists. `/admin/settings/` stays free for the overview (5.4), so add no redirect.
- Checks: the route test lists `/admin/general/`; the menu shows General under Settings; grep finds no `/admin/settings/` left in JS or templates; the e2e search step's settings result now lands on `/admin/general/#…`.
- Commit: `refactor(admin): rename settings page to general`

**5.2 Split notifications from General** · **H**

- Do: move the ntfy (`#alerts-form`), email (`#email-form`) and delivery fields out of `general.html` into a new `notifications.html`: slug `notifications`, label "Notifications", icon `send`, after Backup in `PAGES`/`PAGE_SLUGS`. Keep the handlers and API calls unchanged. Update every `#alerts-form` and `#email-form` link (`BELL`, `alerts.html`, `SETUP_GAPS`, `SETTING_FIELDS`) to `/admin/notifications/…`. No restyle.
- Checks: Notifications saves ntfy and email as before (the existing e2e alert steps now `openPage('notifications')`); General no longer holds those ids; the menu lists Notifications.
- Commit: `refactor(admin): split notifications from general`

**5.3 Merge restore into backup** · **H**

- Do: move `restore.html`'s content into `backup.html` as a section (keep its ids and API calls). Delete the `restore` page from `PAGES`/`PAGE_SLUGS` and delete `restore.html`. Make `/admin/restore/` redirect to `/admin/backup/` and keep the query, like the `/admin/audit/` redirect in `lib.rs`. Update the setup form's "Restore it instead" link and the Backup page link. Add the redirect to `REDIRECTS` in `tests/routes.rs` and to the redirect test.
- Checks: GET `/admin/restore/` redirects; the Backup page has the restore section; the menu has no Restore item; the e2e restore steps pass from the Backup page.
- Commit: `refactor(admin): merge restore into backup`

**5.4 Add the Settings overview** · **H** (if it fails twice: S)

- Do:
  - Add page `settings`, label "Settings", group Settings, icon `settings`, first in the Settings run of `PAGES`/`PAGE_SLUGS`, with template `settings.html` in the Index layout (board `P3-Settings`).
  - In `menu()`, a group whose first page's slug equals the group's lowercase name (`settings`) uses that page as the heading link and does not list it again as a sub-item; the heading is current (`aria-current`) on the overview. Update the `pages.rs` frame test for that one case and explain it in the commit body.
  - One row per section, in rule 1 order: Providers, Clients, Services, Network, Security, Backup, Notifications, General. Each row is a link, its icon, one current-state line, filled by a single `ui.live` read through `textContent`. Use this table:

    | Row | Read | State line |
    | --- | --- | --- |
    | Providers | GET `/providers` | "N providers" |
    | Clients | GET `/users`, `/clients` | "N people, M clients" |
    | Services | GET `/services` | the health line's text |
    | Network | GET `/network` | host name, or "No host name" |
    | Security | GET `/keys` | "API key: name" or "No API key" |
    | Backup | GET `/backup/schedule` | last backup time via `ui.time`, or "Never" |
    | Notifications | GET `/alerts` settings | "ntfy on/off, email on/off" (use whatever field the page already reads) |
    | General | GET `/settings` | "Kill switch on/off" |

  - A failed read shows "Unavailable" on that row only.
- Checks: eight rows with working links; each state line fills; a faked 500 on one read leaves the others; the menu's Settings heading opens the overview and is current there; axe clean.
- Commit: `feat(admin): add the Settings overview`

**5.5 Rebuild Providers and models** · **S**

- As in Appendix B, step 5.5:
  - provider table with Test and Edit;
  - the side panel for Add and Edit: Name, Type, Base URL, write-only key, test model, cost; footer Cancel and "Add and test", with the test running after the save;
  - the CLIProxyAPI accounts move here from Services;
  - the localhost paste step shows only when no domain is set or a login didn't finish;
  - a two-pane model picker with a save bar (`ui.trackChanges`), button "Save".
- Also: `#add-provider` in the address opens the Add panel (Home's quick action links there).
- Checks: as in the original plan, plus the existing provider, billing and hostile-text e2e steps keep their meaning.
- Commit: `feat(admin): rebuild providers and models page`

**5.6 Rebuild People and clients** · **S**

- As in Appendix B, step 5.6:
  - slug `clients` becomes `people`, label "People & clients";
  - `/admin/clients/` redirects to `/admin/people/` (bookmarks; same pattern as 5.3);
  - a users table with Pause and a ⋯ menu (Edit, Delete);
  - Edit opens the side panel with the usage limits greyed;
  - Delete is a typed-name `ui.confirm` with "Pause instead";
  - the pairing box keeps the live "Connected" switch built in 3.6;
  - the pairing key time limit is read from constants.
- Update every `/admin/clients/` address (`SETUP_GAPS`, Home, search, `base.html` setup links).
- Commit: `feat(admin): rebuild people and clients page`

**5.7a Rebuild Services** · **H**

- Do: each service row restarts through `ui.rowConfirm` (the row opens with "Restart <name>?", Cancel and Restart at its bottom right, and the row's own Restart button hides until it closes). Remove the restart-history list and the footer text. Rows render through `ui.rows` with `ui.status` word and symbol. Keep `ui.live` and the 3.6 behaviour. Board `P3-Services`.
- Checks: restart needs the row confirmation; Cancel brings the button back and focuses it; no "recent" list; the existing Services e2e step is updated to the new flow, keeping its meaning.
- Commit: `feat(admin): rebuild services page`

**5.7b Rebuild Network** · **H**

- Do: rebuild the form with the `field` macro (label above, one-line hint under each field, no placeholders as hints, rule 10). The submit button says "Save and apply" (rule 21). Keep the lock-out warning text on screen (rule 12). Use `ui.formErrors` for a failed save. Keep every field id the e2e network steps use. Board `P3-Network`.
- Checks: the network e2e steps pass unchanged in meaning; no placeholder-only hints; the button text is "Save and apply".
- Commit: `feat(admin): rebuild network page`

**5.7c Rebuild Security** · **S**

- As in Appendix B, step 5.7 security part:
  - the password pair through `new_secret` (minimum from constants, live count through `ui.lengthHint`);
  - passkey and recovery codes greyed;
  - the API key table with one key, and a new key shown once in a box (`dialog_box` with `secret`);
  - sealed-content times;
  - audit-key rotation with a stacked passphrase pair.
- Commit: `feat(admin): rebuild security page`

**5.8a Rebuild Backup** · **H**

- Do: the backup passphrase pair through `new_secret` (minimum `MIN_PASSPHRASE_CHARS`); the backup-key rotation keeps its flow; the restore section from 5.3 shows only on a new install. Find the signal the restore flow already uses to refuse a non-empty install. If there is no GET for it, keep the section visible with its existing refusal and report that. Board `P3-Backup`.
- Checks: the backup and restore e2e steps keep their meaning; the passphrase hint becomes a live count.
- Commit: `feat(admin): rebuild backup page`

**5.8b Rebuild Notifications** · **H**

- Do:
  - ntfy and email sections with the `field` macro;
  - a "Page-wide banners" section: one checkbox per banner kind (offline, rejected, chain, alerts, unsealed), the whole section greyed with `na` ("Not available yet: no admin setting stores this"), and the critical ones shown checked and `aria-disabled`;
  - no list of alert kinds on the page (it moves to the reference in 7.1). Board `P3-Notifications`.
- Checks: the ntfy and email e2e steps pass; the critical checkboxes are checked and `aria-disabled`, not `disabled`; no alert-kind list.
- Commit: `feat(admin): rebuild notifications page`

**5.8c Rebuild General** · **H**

- Do: field labels 0.875rem/600 via the `field` macro; a save bar via `ui.trackChanges` ("1 unsaved change: <label>", Discard, Save); a "Danger zone" section holding the kill switch, which saves only after `ui.confirm`; ranges come from constants. Keep the field ids the settings e2e steps use. Board `P3-General`.
- Checks: the save bar count follows the number of changed fields; Discard restores; the kill switch asks first; the status bar's `#sb-kill` follows within one status poll.
- Commit: `feat(admin): rebuild general page`

## Phase 6: Audit, sign-in and setup

**6.1 Rebuild Audit / Model calls** · **S**

- As in Appendix B, step 6.1:
  - a KPI strip;
  - filters (unsupported ones greyed);
  - a table with one-line cells;
  - the selected row's trace opens in the docked side panel (`ui.openPanel` with `param: 'id'`), and `?id=` opens it on load;
  - Back and Esc close it;
  - the list goes onto `ui.live` (deferred from 3.6) with load-more kept;
  - Audit filters in the address (`?status=`, `?provider=`), so global search's Audit rows can link straight to them; update `SETTING_FIELDS`/search to use them.
- `audit.html` now serves Model calls only.
- Commit: `feat(admin): rebuild audit model calls with docked trace`

**6.2 Add the Admin actions, Sealed access and Routing pages** · **H**

- Do:
  - give `audit/sealed` and `audit/routing` their own templates (`audit_sealed.html`, `audit_routing.html`) by moving their markup out of `audit.html`, and drop the in-page sub-nav;
  - `audit.js` loads the right view per `body[data-page]`;
  - add a tab bar to all four Audit pages with the `tabs` macro changed to links: four `<a>` with `aria-current="page"` on the current one, labels and order equal to the menu;
  - Admin actions keeps its "Not available yet" note and adds a greyed table head with `aria-disabled`.
- Boards: `P3-Audit-Admin`, `P3-Audit-Sealed`, `P3-Audit-Routing`.
- Checks: the four tabs equal the four menu links in order; each page marks its own; the sealed and routing e2e steps open their pages directly and keep their meaning.
- Commit: `feat(admin): add audit admin actions, sealed access and routing pages`

**6.3 Restyle the sign-in card** · **H**

- Do: in `base.html`'s `#login`, use the card layout from board `P3-Signin`: the logo inline with "Sign in to Askbar", 40 px buttons (`btn-l`), the first-run hint shown only until the default password is changed, and no attempts warning. Find the status field that says whether the published password is still in use; the setup flow already detects it, so read it from there. A failed sign-in error is inline and focused (`ui.formErrors`).
- Checks: the hint is absent after the password changes; no attempts text; "Skip to content" is still the first Tab stop; the sign-in e2e steps pass.
- Commit: `feat(admin): restyle the sign-in card`

**6.4 Restyle first-run setup** · **S**

- As in Appendix B, step 6.4: a card with a three-step progress indicator, password and passphrase pairs through `new_secret` (minimums from constants), and a mismatched pair blocks Next with an inline error.
- Commit: `feat(admin): restyle first-run setup`

## Phase 7: Reference and help

**7.1a Set up docs/reference** · **H**

- Do:
  - create `docs/reference/` with an outline and placeholder pages, each with front matter (`id`, `title`, `summary`, `keywords`, `settings`); one heading per settings section is a help topic id (`settings/network#host-name`);
  - include `admin-ui/components.md`, `admin-ui/page-layouts.md` and `audit/alert-kinds.md` (the list removed from Notifications in 5.8b, copied from the alerter's kinds with the exact names);
  - add `scripts/check-reference.sh`, which checks unique ids, that every link resolves, and that every help topic id listed in `constants.rs` (add a `HELP_TOPICS` constant) exists; call it from `scripts/check-docs.sh`;
  - do not move existing docs here (that is 7.1b).
- Checks: both scripts pass; a broken link or a duplicate id makes the script fail.
- Commit: `docs(reference): set up docs/reference outline and check script`

**7.1b Move the server sections into the reference** · **S** · **ask the owner first**

- As in Appendix B, step 7.1 "MOVE, not copy", only after the owner agrees to move parts of `docs/architecture.md` and `docs/guide.md`.
- Every link to a moved section is updated, AGENTS.md's references still resolve, and `check-docs.sh` passes.
- If the owner says no, skip this step and leave the reference pages linking to the existing docs.
- Commit: `docs(reference): move the server sections into docs/reference`

**7.2 Render the reference in the admin** · **S** · **dependency approval first**

- As in Appendix B, step 7.2: `build.rs` with `pulldown-cmark` (build dependency, raw HTML off) renders each page and a JSON index; `/admin/reference/` and `/admin/reference/<path>/` use the Reference article layout; "Edit on GitHub" is built from `SOURCE_URL`.
- State the dependency and its MIT license in the commit body; `cargo deny check` must pass.
- Commit: `feat(admin): render the reference in the admin`

**7.3 Add help buttons and help in the side panel** · **S**

- As in Appendix B, step 7.3:
  - `help(topic)` after each page title and settings section and in side-panel titles;
  - it opens the side panel with the topic's text pre-rendered at build time, plus "Open full page";
  - a Rust test fails on a topic missing from the index.
- This is also when pages move onto the `page_header` macro (title, `?`, one-line lede, actions), which steps 4.1 to 6.x deferred; update the `pages.rs` frame test.
- Commit: `feat(admin): add help buttons and help in the side panel`

**7.4 Search the reference** · **H**

- Do: fill the empty "Reference" group that `ui.combobox` already reserves (3.5) from the index 7.2 produces; it is always last. Enter on a topic that is on the current page opens the help panel (7.3's opener); any other topic opens the article. Keep the cap at `SEARCH_RESULTS_PER_GROUP`.
- Checks: "host name" lists the Reference group last; Enter on an on-page topic opens the panel; on another topic, the article.
- Commit: `feat(admin): search the reference`

**7.5 Show the live components in the reference** · **S**

- As in Appendix B, step 7.5.
- Commit: `feat(admin): show live components in the reference`

**7.6 Mirror the reference to the wiki** · **H** · **only if the owner said yes**

- As in Appendix B, step 7.6. It changes `.github/workflows/`, which AGENTS.md says needs asking.
- Commit: `ci(docs): mirror the reference to the wiki`

## Phase 8: Cleanup and the style guide

**8.1 Carry-forward fixes** · **H**

- Do:
  - On a phone, the kill switch shows its word again (rule 37: word plus symbol). Shorten it ("Kill: off") or move it into the phone menu.
  - At 320 px wide, nothing scrolls sideways: let the status bar's facts wrap or drop the least important. Add a 320 px check to the existing "status bar" e2e step (a one-off resize, not a third suite width).
- Commit: `fix(admin): keep the phone status bar complete and inside 320 px`

**8.2 Remove the replaced code** · **H**

- As in Appendix B, step 8.1: old `.top` nav CSS, the flash CSS (keep `.flash` only while `#sched-reminder` uses it, or move that onto `notice`), `restore.html` if still present, the old Audit detail pane CSS, the unused `blocks.html` macros, and JS that only served them.
- Checks: grep finds none of the removed selectors; all tests pass.
- Commit: `chore(admin): remove replaced code`

**8.3 Tidy the tests and add the drift checks** · **H**

- As in Appendix B, step 8.2, plus:
  - The literal check uses an allowlist for time-unit maths (`60`, `1000`, `3600`, `24`).
  - It also checks page addresses: no `/admin/<slug>/` in JS or templates for a slug not in `PAGE_SLUGS` or the redirects.
  - It checks that every greyed item contains no digit.
- Commit: `test(admin): rename steps and add drift checks`

**8.4 Update docs/style/ui-ux.md** · **H**

- As in Appendix B, step 8.3.
- Also record:
  - why the current page's name is `--fg` (accent on the accent tint is under 4.5:1);
  - the fixed menu tooltip;
  - that history goes in the bell and Alerts, not in banners.
- Commit: `docs(style): update the admin UI/UX rules`

**8.5 Turn audit-shots into a page screenshot script** · **H**

- As in Appendix B, step 8.4.
- Commit: `test(admin): turn audit-shots into a page screenshot script`

## Follow-ups (not in this plan; need the owner or server work)

From the original plan (Appendix C), still open:

- family budget and per-person usage and caps
- user rename
- per-model stats and per-provider latency
- service uptime
- Audit user/model/space filters, search and CSV
- reading the admin-action log and alert history
- sealed-session end times
- a setting that stores banner choices
- passkeys and new recovery codes
- more backup destinations and a backup time
- certificate expiry

Added by phases 3–4:

- **Dismissing alerts is per browser.** Alerts dismissed on the Alerts page are kept in localStorage until the server can store dismissals.
- **List ETags.** The admin API sends no ETag on list endpoints, so `ui.live` compares bodies in the browser. Server ETags would save the transfer.
- **Alert start times.** The status response gives no start time, so the Alerts page shows "ongoing" instead of "Since 14:31".
- **Intermittent 500 in "server away" steps.** It looks like `admin_api::mutate` leaves `BEGIN IMMEDIATE` open when a request is cancelled during `push_snapshot`, after which every change fails until restart. A rollback-on-drop guard in `mutate` would fix it. This is server code: propose it to the owner as its own `fix(admin)` change with a test that cancels a mutate mid-push.

## Appendix A: binding sections of the original plan

Copied verbatim from the Nov 15 plan.

### Scope and constraints

**In scope:** the admin's templates, CSS, scripts, `constants.rs`, `build.rs` for the reference, their tests, `docs/reference` and `docs/style/ui-ux.md`.

**Out of scope:** the admin API, the server, model-gateway, the auditor, the alerter and the desktop client. The shared `tokens.css` only gains new variables.

**Rules every step keeps:** the "Admin UI/UX rules" below (1–48), plus the single-source-of-truth rule. Subagents get both sections in full.

**Not available yet.** These are drawn but disabled until the server supports them (rule 40):

| Element | Why it's disabled |
| --- | --- |
| Family budget ("Budget 38%", "of $20") | No setting for a family budget |
| Spend by person, token caps, Edit user name | No per-person usage, caps or rename |
| Models used chart, per-provider latency | Stats are grouped by provider only |
| Service uptime | Not reported by the server |
| Audit: User/Model filters, search, Space column, CSV export | Not in the audit API |
| Audit: Admin actions page, alert History | Admin can't read these logs back |
| Sealed access: when a session ended and how | Only openings are recorded |
| Page-wide banner choices | No admin setting to store them |
| Passkey sign-in, new recovery codes | Not built |
| Backup destinations other than `/backup`, backup time | Not in the backup API |
| Certificate expiry date | Not reported |

### Single source of truth: constants

Every value the UI depends on is defined once, the way Odysseus's `src/constants.py` works: one module, imported everywhere, never re-derived.

- **`services/admin/src/constants.rs` (step 1.1)** holds the behaviour values and limits. Existing constants move there or are re-exported from it, so the server's checks and the UI's checks read the same value:

| Group | Constants (examples) |
| --- | --- |
| Identity and links | `ADMIN_USERNAME`, `API_KEY_PREFIX` (`askbar-admin-`), `SOURCE_URL`, `LICENSE` (`AGPL-3.0-or-later`), `REFERENCE_ROOT` (`docs/reference`) |
| Sign-in and secrets | `MIN_PASSWORD_CHARS` (10), `MAX_PASSWORD_CHARS` (200), `MIN_PASSPHRASE_CHARS` (20), `SESSION_TTL_SECS`, `MAX_FAILURES`, `LOCKOUT_SECS`, `FRESH_SIGNIN_SECS` (600) |
| Settings ranges | `GENERATION_TIMEOUT_SECS` (30–3600), `AUDIT_RETENTION_DAYS` (7–3650), `UNSEAL_MINUTES` (defaults, max 720), `MAX_HISTORY_TURNS`, `MAX_NOTICE_CHARS`, `MAX_WARNING_CHARS`, `API_KEY_MAX_NAME_CHARS` |
| Pairing and services | `PAIR_CODE_TTL_SECS` (600), `SERVICES`, `MAX_RESTARTS_PER_MINUTE` (6) |
| Backups | `BACKUP_RUN_AT`, `BACKUP_RETRY_AFTER_SECS`, `DRILL_EVERY_SECS` |
| UI timing | `STATUS_EVERY_SECS` (5), `LIVE_EVERY_SECS` (10), `HIDDEN_EVERY_SECS` (60), `BACKOFF_SECS` (10, 20, 40, 60), `HIGHLIGHT_SECS` (3), `TOAST_SECS` (5) |
| Pages and help | the page ids and groups used by `PAGES`, the help topic ids, the reference paths |

- **Getting them to the browser:** `/admin/constants.json` is generated from `constants.rs` with serde and served from the same origin with an ETag. `components.js` loads it before `app.js` and exposes it as `ui.constants`. Templates get the same values through Askama. No number, limit or URL is typed into `app.js`, `audit.js`, `components.js` or a template.
- **What stays in `tokens.css`:** visual values only, such as colours, sizes, spacing, radii, the 440 px panel width, the 48 px status bar and the 216/64 px menu.
- **Drift checks:** step 8.2 fails the build on any literal in the scripts or templates for a value that `constants.json` defines. Step 1.1 tests that every constant appears in the JSON.

### Admin UI/UX rules

Source of truth for every step. Step 8.3 copies this section into `docs/style/ui-ux.md` ("The admin site"), replacing the rules it supersedes.

**Navigation**

1. The side menu links every page: Home, Alerts, Audit (Model calls, Admin actions, Sealed access, Routing), Settings (Providers & models, People & clients, Services, Network, Security, Backup & restore, Notifications, General) and Reference. The current page is highlighted and its parent is bold.
2. The menu collapses to icons (64 px); the choice is remembered per browser, and names show on hover and on keyboard focus. Phones get a top bar with a Menu button that opens a full-screen menu.
3. The status bar (48 px) on every page holds the breadcrumb, global search, family budget, kill switch, seal toggle and alerts bell.
4. Global search covers pages, settings (with their current values), people, clients, providers, models and Audit filters; reference results always come last, under "Reference". "/" focuses it.

**Page structure**

5. Every page is one of eight layouts: Dashboard, List, Settings form, Index, Signed-out card, Reference article, Side panel (docked) and Box.
6. Every page header has a title, a ? help button, one line of description, and its actions on the right; the title sits 24 px below the status bar.
7. Hairlines between sections, not boxes. The card style is used only on sign-in and first-run setup.
8. One filled primary button per view.

**Text and help**

9. Each settings section has its name, a ? button and at most one line of help.
10. A field hint is one line under the field (format, range, limit); never placeholder text and never in brackets in the label.
11. Explanations live in the reference; the ? button opens the side panel at that topic, with "Open full page".
12. Text stays on screen only if it prevents a mistake or is needed to finish a task: lock-out warnings, "cannot be recovered", required steps.
13. Reference text is Markdown in `docs/reference`, written once and shown in the admin, its search and on GitHub.

**Type, spacing and colour**

14. Sizes: 1.3rem page title, 0.9375rem section heading, 0.875rem labels and body, 0.75rem help, captions and table headers.
15. Weights 400 and 600 only.
16. Spacing 4–32 px; radius 8 or 16 px.
17. Visual values come only from the shared `tokens.css`; behaviour values and limits come only from `constants.rs` (see "Single source of truth").

**Buttons**

18. Heights 28 (small), 32 (default), 40 (sign-in and setup), at least 44 on phones; width fits the label.
19. Filled primary, outline secondary, red outline destructive.
20. Icon-only buttons only for close, collapse, bell, copy, sign out and help, each with a name and tooltip.
21. Save buttons say "Save"; "Save and apply" only where saving takes effect immediately (Network).

**Forms**

22. Label above the field; mark "(optional)" when most fields are required.
23. Field labels 0.875rem/600.
24. Password and passphrase pairs are stacked full width (new, then repeat), with the length rule under the first field; it becomes a live count as you type. Paste is always allowed.
25. A save bar appears once something changes: "1 unsaved change: …" with Discard and Save.
26. A failed save shows a summary at the top and moves focus to the first error.

**Panels, boxes and confirmations**

27. One side panel, docked on the right under the status bar, for adding, editing, help and the selected Audit row's details. The page narrows and stays usable; nothing is dimmed.
28. One panel at a time, 440 px wide. Esc and × close it; unsaved changes ask first; it has its own address so Back closes it.
29. Boxes are modal and used for confirmations and one-off secrets (pairing key, new API key).
30. A risky action on one row (restart a service) opens that row to confirm: Cancel and the action at its bottom right; the row's own button hides until it closes.
31. Deleting: no confirmation when Save or Undo covers it; a confirmation box when it can't be undone; typing the name when it destroys data or affects everyone.
32. Toasts are for success and Undo only; errors are inline, next to the fix.

**Lists and data**

33. One visible row action plus a ⋯ menu; Delete last, in red.
34. Lists refresh every 10 s while the tab is visible: new rows are highlighted and announced; scroll, focus and typing are kept; the admin's own changes show at once.
35. While the tab is hidden, the bell and tab title still refresh once a minute; failed checks back off and show "Reconnecting…".
36. Relative times with the full date on hover; numbers right-aligned with units.
37. Statuses use a fixed set of words, always a word plus a symbol, never colour alone.

**Alerts**

38. The bell badge counts critical plus warning; its menu groups Critical, Warning and Info; a new critical alert also pops up.
39. Page-wide banners only for conditions that affect the whole admin; they can't be dismissed and clear themselves; which alerts get one is set in Notifications, and critical ones are always shown.

**Server capabilities**

40. A feature the server can't support yet is greyed out with `aria-disabled` (never `disabled`): Tab reaches it, Enter does nothing, and a "Not available yet: …" note shows on hover and focus.

**Accessibility (WCAG 2.2 AA)**

41. Everything works by keyboard; visible focus ring never hidden behind sticky bars; "Skip to content" is the first Tab stop.
42. Every control has a name; tooltips also show on keyboard focus.
43. Contrast 4.5:1 for text and 3:1 for icons and field borders, in both themes.
44. Targets at least 24 px on desktop and 44 px on phones.
45. One announcer: updates are read out without moving focus; polite, except critical alerts and banners.
46. Charts carry a one-line text summary; nothing animates under reduced motion.

**Security limits**

47. No inline script or style; text from the server goes in through `textContent`.
48. Help and reference text is rendered at build time into the page, never injected at run time (Trusted Types stays 'none').

### Target structure

Every page shares one frame: the side menu, the status bar and the page header.

- **Home**
- **Alerts**
- **Audit:** Model calls · Admin actions · Sealed access · Routing (each its own page; `/admin/audit/` opens Model calls)
- **Settings:** the overview, then Providers & models · People & clients · Services · Network · Security · Backup & restore (Restore merged in) · Notifications · General
- **Reference:** the index and one article per page in `docs/reference`
- **Signed out:** Sign in and First-run setup (card layout, no menu)

**Files touched:**

- `services/admin/src/` (`constants.rs`, `pages.rs`, `lib.rs`, `build.rs`, the `reference` module)
- `ui/` (templates, `components.html`, `style.css`, `components.js`, `app.js`, `audit.js`, both `tokens.css` copies)
- `tests/e2e/admin.test.js`
- `docs/reference/`
- `docs/style/ui-ux.md`

There are no API changes and no changes to other services.

### Component library

Every repeated element is built from one shared template, so pages never hand-write its markup and a fix lands everywhere at once.

- **Server-rendered parts:** Askama macros in `templates/components.html`.
- **Parts built from API data:** small builders in `ui/components.js`, which build the DOM with `textContent` so the CSP and Trusted Types rules hold.

The visual spec is "Reusable components — reference" on page 3.

| Component | Template (macro) | Script builder | Used for |
| --- | --- | --- | --- |
| Page header | `page_header(title, lede, help, actions)` | — | Every page |
| Help button | `help(topic)` | `ui.help(topic)` | After every page title, settings section and side-panel title (not in boxes) |
| Button, icon button | `button(label, kind, size, icon)`, `icon_button(icon, label)` | `ui.button()` | All actions |
| Form field | `field(label, id, hint, type, optional)` | `ui.fieldError(id, text)` | All forms |
| New password or passphrase | `new_secret(label, min)` | `ui.lengthHint(input, min)` | Admin password, backup and audit passphrases, setup |
| Save bar | `save_bar(form)` | `ui.trackChanges(form)` | Settings forms, model picker |
| Side panel (docked) | `side_panel(id, title, help)` | `ui.openPanel(id)` | Add and edit users and providers, help, the selected Audit row |
| Box | `box(id, title)` | `ui.openBox(id)` | Confirmations, pairing key, new API key |
| Confirmation | — | `ui.confirm({title, body, action, typeName})` | Revoke, delete, restore, kill switch |
| Inline row confirmation | `row_confirm(id, text, action)` | `ui.rowConfirm(row, opts)` | Restarting a service |
| Toast | `toast_region()` (once, in the frame) | `ui.toast(text, {undo})` | Success and Undo only |
| Announcer, skip link | `announcer()`, `skip_link()` (once, in the frame) | `ui.announce(text, {urgent})` | Screen-reader updates |
| Inline notice | `notice(kind, text, fix)` | `ui.notice()` | Errors with the fix beside them |
| Page-wide banner | `banner_region()` (once, in the frame) | `ui.banner(kind, text)` | Offline, token rejected, kill switch, sealed open |
| Status label | `status(state)` | `ui.status(state)` | Services, providers, users, clients |
| Table and row actions | `table_head(cols)`, `row_menu(items)` | `ui.rows(tbody, items, render)`, `ui.rowMenu(items)` | Every list |
| Empty state, loading | `empty(text, action)`, `skeleton(rows)` | `ui.empty()`, `ui.busy(button, text)` | Lists and saving buttons |
| Copy and show-once secret | `secret(id)` | `ui.copy(button, value)` | Pairing keys, API keys |
| Time and number | — | `ui.time(ts)`, `ui.num(n, unit)` | Relative time with full date on hover; right-aligned numbers |
| Tabs, menu | `tabs(items)`, `menu(id)` | `ui.tabs()`, `ui.menu(button, items)` | Audit tabs, bell, row menus |
| Not available yet | `na(reason)`, `na_tag(reason)` | — | Greyed-out elements |
| Live list | — | `ui.live(loader, opts)` | Home, Alerts, People and clients, Services, Providers |
| Constants | — | `ui.constants` | Every script |

## Appendix B: original text of phases 5–8

Copied verbatim from the Nov 15 plan. The steps above split or adjust these; where they differ, the steps above win.

#### Phase 5: Settings

Every Settings section becomes its own page in the Settings form or Index layout, built from the shared components.

**5.1 Rename the settings page to general** · Model: Haiku

- Goal: Rename the existing `settings` page to `general` (slug, label "General", template `general.html`, `page!` line, `PAGES` entry) and move its page id into constants.rs. `/admin/settings/` stays reserved for the overview (5.4), so add no redirect from it here. Update the menu link and the route-table test. Matches "Settings / General — desktop, light" only for the name; the rebuild is 5.8.
- Files: services/admin/src/pages.rs, services/admin/src/lib.rs, services/admin/src/constants.rs, services/admin/src/ui/templates/settings.html (renamed to general.html), tests/e2e/admin.test.js
- Checks: route-table test lists `/admin/general/`; menu shows General under Settings; page id read from constants.rs; tests pass at 1280 and 390.
- Commit: `refactor(admin): rename settings page to general`

**5.2 Split notifications from General** · Model: Haiku

- Goal: Move the ntfy, email and alert-delivery fields out of `general.html` into a new `notifications.html` page (slug `notifications`, label "Notifications") using the same JS handlers and API calls unchanged. Add the `page!` line and `PAGES` entry in menu order. No restyle yet; 5.8 rebuilds it.
- Files: services/admin/src/pages.rs, services/admin/src/ui/templates/general.html, services/admin/src/ui/templates/notifications.html, services/admin/src/ui/app.js, tests/e2e/admin.test.js
- Checks: Notifications page saves ntfy and email as before; General no longer contains those fields; menu lists Notifications; tests pass at 1280 and 390.
- Commit: `refactor(admin): split notifications from general`

**5.3 Merge restore into backup** · Model: Haiku

- Goal: Move the restore form into `backup.html` as a section, delete the `restore` page from `PAGES`, and make `/admin/restore/` answer with a redirect to `/admin/backup/`. Keep the restore API calls unchanged. The "new install only" rule comes in 5.8.
- Files: services/admin/src/pages.rs, services/admin/src/lib.rs, services/admin/src/ui/templates/backup.html, services/admin/src/ui/templates/restore.html, services/admin/src/ui/app.js, tests/e2e/admin.test.js
- Checks: GET `/admin/restore/` returns a redirect to `/admin/backup/`; Backup page contains the restore section; menu has no separate Restore link; tests pass at 1280 and 390.
- Commit: `refactor(admin): merge restore into backup`

**5.4 Add the Settings overview** · Model: Sonnet

- Goal: Add `/admin/settings/` in the Index layout: one row per settings section (Providers & models, People & clients, Services, Network, Security, Backup & restore, Notifications, General), each linking to its page with one current-state line (for example the provider count, the domain, last backup time) filled in by `ui.fetchJson` through `textContent`, with page header, help button and description per rule 6. Matches "Settings — desktop, light".
- Files: services/admin/src/pages.rs, services/admin/src/ui/templates/settings.html, services/admin/src/ui/app.js, services/admin/src/constants.rs, tests/e2e/admin.test.js
- Checks: eight rows with working links; each state line shows a value after load; a failed lookup shows "Unavailable" for that row only; no literal for page ids outside constants.rs; tests pass at 1280 and 390.
- Commit: `feat(admin): add the Settings overview`

**5.5 Rebuild Providers and models** · Model: Sonnet

- Goal: Rebuild the page. Providers table with Test and Edit per row; Add and Edit open the docked side panel (`ui.openPanel`) with Name, Type, Base URL, write-only API key, test model and cost; footer Cancel and "Add and test", where the test runs after the save. Move the Claude and ChatGPT accounts here from Services; the "paste the http://localhost address" step and box show only when no domain is set or a login started from a domain did not finish. Two-pane model picker with a save bar labelled Save. Matches "Settings / Providers & models — desktop, light"; page B: "Add provider panel".
- Files: services/admin/src/ui/templates/providers.html, services/admin/src/ui/templates/services.html, services/admin/src/ui/app.js, services/admin/src/ui/style.css, tests/e2e/admin.test.js
- Checks: Add saves then tests; API key never shown after save; localhost step hidden when a domain is set and no login is pending, shown otherwise; picker save bar appears on change and is labelled Save; accounts no longer on Services; Esc with unsaved panel input asks first; tests pass at 1280 and 390.
- Commit: `feat(admin): rebuild providers and models page`

**5.6 Rebuild People and clients** · Model: Sonnet

- Goal: Rename the clients page to People and clients (slug `people`). Users table with Pause and a ⋯ menu (Edit, Delete). Edit opens the docked side panel with the usage limits greyed (`aria-disabled`, "Not available yet: ..."). Delete is a type-the-name box with "Pause instead". The "Pair a new client" box shows the key, then switches to Connected using the live list from 3.6. Matches "Settings / People & clients — desktop, light"; page B: "Edit user", "Delete user", "Pair a new client box", "Pair a new client, connected".
- Files: services/admin/src/pages.rs, services/admin/src/ui/templates/clients.html, services/admin/src/ui/app.js, services/admin/src/ui/style.css, tests/e2e/admin.test.js
- Checks: Delete button stays disabled until the name matches; "Pause instead" pauses the user; pairing box turns to Connected when the client appears; greyed limits are reachable by Tab and do nothing on Enter; pairing key TTL read from constants; tests pass at 1280 and 390.
- Commit: `feat(admin): rebuild people and clients page`

**5.7 Rebuild Services, Network and Security** · Model: Sonnet

- Goal: Services: inline row confirmation (the row opens, Cancel and the action at its bottom right, the row's button hidden; `ui.rowConfirm`), no restart history, no footer text. Network: "Save and apply", one-line hints, lock-out warning kept. Security: stacked password pair (`new_secret`, minimum from constants), passkey and recovery codes greyed, API key table with one key, sealed-content times, audit-key rotation with a stacked passphrase pair. Matches "Settings / Services — desktop, light", "Settings / Network — desktop, light" and "Settings / Security — desktop, light".
- Files: services/admin/src/ui/templates/services.html, services/admin/src/ui/templates/network.html, services/admin/src/ui/templates/security.html, services/admin/src/ui/app.js, services/admin/src/ui/style.css, tests/e2e/admin.test.js
- Checks: restart needs the row confirm and Cancel restores the button; password minimum comes from constants.json and the hint becomes a live count; new API key appears in a box once; rotation needs matching passphrases; no new literal for limits; tests pass at 1280 and 390.
- Commit: `feat(admin): rebuild services, network and security pages`

**5.8 Rebuild Backup, Notifications and General** · Model: Sonnet

- Goal: Backup: stacked passphrase pair, restore shown only on a new install. Notifications: ntfy, email and page-wide banner choices (critical locked on), with no list of alert kinds (it moves to the reference in 7.1). General: field labels 0.875rem/600, save bar "1 unsaved change: ..." with Save, Danger zone with the kill switch. Matches "Settings / Backup & restore — desktop, light", "Settings / Notifications — desktop, light" and "Settings / General — desktop, light".
- Files: services/admin/src/ui/templates/backup.html, services/admin/src/ui/templates/notifications.html, services/admin/src/ui/templates/general.html, services/admin/src/ui/app.js, services/admin/src/ui/style.css, tests/e2e/admin.test.js
- Checks: restore section hidden once the install has data; critical banner checkbox is checked and aria-disabled; no alert-kind list on the page; save bar count follows the number of changed fields; kill switch asks for confirmation; ranges from constants; tests pass at 1280 and 390.
- Commit: `feat(admin): rebuild backup, notifications and general pages`

#### Phase 6: Audit, sign-in and setup

The Audit area becomes four linked pages, and the two signed-out screens use the new card style.

**6.1 Rebuild Audit / Model calls** · Model: Sonnet

- Goal: Rebuild the page with a KPI strip, filters (unsupported ones greyed with `aria-disabled`) and a table with one-line cells. The selected row's trace opens in the docked side panel (title, status, ×) instead of a separate pane, and the URL keeps `?id=`, so Back closes the panel. Lists refresh every 10 s from constants. Matches "Audit / Model calls — desktop, light".
- Files: services/admin/src/ui/templates/audit.html, services/admin/src/ui/audit.js, services/admin/src/ui/style.css, tests/e2e/admin.test.js
- Checks: selecting a row opens the panel and sets `?id=`; loading `?id=` opens it; Esc and Back close it; old detail pane removed from the markup; greyed filters show "Not available yet" on focus; tests pass at 1280 and 390.
- Commit: `feat(admin): rebuild audit model calls with docked trace`

**6.2 Add the Admin actions, Sealed access and Routing pages** · Model: Sonnet

- Goal: Add the three pages from 2.1, each with its own template and `page!` entry. The tab bar on every Audit page matches the menu links. Admin actions is greyed (not readable yet) with a "Not available yet: ..." note. Matches "Audit / Admin actions — desktop, light", "Audit / Sealed access — desktop, light" and "Audit / Routing — desktop, light".
- Files: services/admin/src/pages.rs, services/admin/src/ui/templates/audit\_admin.html, services/admin/src/ui/templates/audit\_sealed.html, services/admin/src/ui/templates/audit\_routing.html, services/admin/src/ui/audit.js, tests/e2e/admin.test.js
- Checks: four tabs equal the four menu links in order; each page highlights its own tab; Admin actions table is aria-disabled; route-table test lists all four paths; tests pass at 1280 and 390.
- Commit: `feat(admin): add audit admin actions, sealed access and routing pages`

**6.3 Restyle the sign-in card** · Model: Sonnet

- Goal: Restyle the Signed-out card layout: logo inline with "Sign in to Askbar", 40 px buttons, the first-run hint shown only until the default password is changed, and no attempts warning. Matches "Sign in — desktop, light".
- Files: services/admin/src/ui/templates/base.html, services/admin/src/ui/style.css, services/admin/src/auth.rs, tests/e2e/admin.test.js
- Checks: hint absent after the password changes; no attempts text; failed sign-in error is inline and focused; "Skip to content" still first Tab stop; tests pass at 1280 and 390.
- Commit: `feat(admin): restyle the sign-in card`

**6.4 Restyle first-run setup** · Model: Sonnet

- Goal: Restyle setup as a card with a three-step progress indicator. Password and passphrase pairs are stacked full width via `new_secret`, with minimums from constants. Matches "First-run setup — desktop, light".
- Files: services/admin/src/ui/templates/base.html, services/admin/src/ui/app.js, services/admin/src/ui/style.css, tests/e2e/admin.test.js
- Checks: progress shows step 1 of 3 through 3 of 3; mismatched pair blocks Next with an inline error; paste works in all fields; minimum from constants.json; tests pass at 1280 and 390.
- Commit: `feat(admin): restyle first-run setup`

#### Phase 7: Reference and help

The explanations move into Markdown under docs/reference, shown in the admin, its search and on GitHub.

**7.1 Set up docs/reference** · Model: Haiku

- Goal: Create `docs/reference` with an outline and placeholder pages, each with front matter (id, title, summary, keywords, settings). One heading per settings section is a help topic id (for example `settings/network#host-name`). MOVE `docs/architecture.md` and the guide's server sections instead of copying. Include `admin-ui/components.md`, `admin-ui/page-layouts.md` and `audit/alert-kinds.md` (the list removed from Notifications). Add a script that checks ids and links.
- Files: docs/reference/, docs/architecture.md, docs/guide.md, scripts/check-reference.sh, scripts/check-docs.sh
- Checks: `scripts/check-reference.sh` passes (unique ids, every link resolves, every id in constants.rs help topics exists); `scripts/check-docs.sh` passes; no duplicate of moved text remains.
- Commit: `docs(reference): set up docs/reference outline and check script`

**7.2 Render the reference in the admin** · Model: Sonnet

- Goal: `build.rs` with `pulldown-cmark` (build dependency, raw HTML off) renders each page to HTML plus a JSON index. Serve `/admin/reference/` and `/admin/reference/<path>/` in the Reference article layout, with "Edit on GitHub" built from the constants SOURCE\_URL. Matches "Reference — desktop, light" and "Reference / Admin settings / Network — desktop, light".
- Files: services/admin/build.rs, services/admin/Cargo.toml, services/admin/src/reference.rs, services/admin/src/pages.rs, services/admin/src/lib.rs, services/admin/src/ui/templates/reference.html, services/admin/src/ui/style.css, tests/e2e/admin.test.js
- Checks: Rust test renders every page without raw HTML passing through; unknown path returns 404; "Edit on GitHub" link uses SOURCE\_URL; CSP unchanged; menu Reference link is highlighted; tests pass at 1280 and 390.
- Commit: `feat(admin): render the reference in the admin`

**7.3 Add help buttons and help in the side panel** · Model: Sonnet

- Goal: Add `help(topic)` after each page title and settings section and in side-panel titles (not in boxes). It opens the same docked side panel (`ui.openPanel`) with the topic's text pre-rendered into the page at build time and an "Open full page" link. A Rust test fails on a topic missing from the index. Matches "Settings / Network, help open — desktop, light".
- Files: services/admin/src/ui/templates/components.html, services/admin/src/reference.rs, services/admin/src/ui/app.js, services/admin/src/ui/style.css, services/admin/src/constants.rs, tests/e2e/admin.test.js
- Checks: every ? button has a name and tooltip; panel shows topic text without a network fetch; "Open full page" goes to the article anchor; Rust test fails for a missing topic; help address closes on Back; tests pass at 1280 and 390.
- Commit: `feat(admin): add help buttons and help in the side panel`

**7.4 Search the reference** · Model: Sonnet

- Goal: Add reference results to global search, always last under "Reference". Enter opens help in the panel when the topic is on the current page, otherwise the article. Matches "Search, with reference results — desktop, light".
- Files: services/admin/src/ui/app.js, services/admin/src/reference.rs, services/admin/src/ui/templates/base.html, tests/e2e/admin.test.js
- Checks: query "host name" lists Reference group last; Enter on an on-page topic opens the panel; Enter on another topic opens the article; "/" still focuses search; tests pass at 1280 and 390.
- Commit: `feat(admin): search the reference`

**7.5 Show the live components in the reference** · Model: Sonnet

- Goal: The `admin-ui` reference pages render the real Askama macros next to the Markdown. Screenshots used on GitHub are generated by 8.4. Matches "Reusable components — reference" and "Page layouts — reference".
- Files: services/admin/src/reference.rs, services/admin/src/ui/templates/blocks.html, services/admin/src/ui/templates/reference.html, docs/reference/admin-ui/components.md, docs/reference/admin-ui/page-layouts.md, tests/e2e/admin.test.js
- Checks: each component named in components.md renders on the page; no hand-written copy of a component's markup in the Markdown; only admin-ui pages show live examples; tests pass at 1280 and 390.
- Commit: `feat(admin): show live components in the reference`

**7.6 Mirror the reference to the wiki** · Model: Haiku

- Goal: Only if the open question says yes: a script publishes `docs/reference` to the GitHub wiki, one way, with an "edit in docs/reference" banner on every page.
- Files: scripts/mirror-wiki.sh, .github/workflows/wiki.yml
- Checks: `scripts/check-reference.sh` passes; dry run prints every page with the banner and no wiki edit is read back; tests pass at 1280 and 390 only if browser tests apply.
- Commit: `ci(docs): mirror the reference to the wiki`

#### Phase 8: Cleanup and the style guide

The replaced code is removed, the tests follow the new pages, and the style guide matches the rules.

**8.1 Remove the replaced code** · Model: Haiku

- Goal: Delete flash banners, the old nav CSS, `restore.html` and the old detail pane CSS, and any JS that only served them.
- Files: services/admin/src/ui/style.css, services/admin/src/ui/app.js, services/admin/src/ui/audit.js, services/admin/src/ui/templates/restore.html
- Checks: grep finds no flash-banner, old nav or detail-pane selectors; `scripts/check-ui-tokens.sh` passes; tests pass at 1280 and 390.
- Commit: `chore(admin): remove replaced code`

**8.2 Tidy the tests and add the drift checks** · Model: Haiku

- Goal: Rename e2e steps to the new pages. Add a check that no template hand-writes markup a component covers. Add a check that app.js, audit.js, components.js and the templates contain no literal for any value defined in constants.rs (grep list generated from constants.json). Assert the grouped menu in pages.rs.
- Files: tests/e2e/admin.test.js, scripts/check-ui-tokens.sh, services/admin/src/pages.rs, services/admin/src/constants.rs
- Checks: `cargo test -p askbar-admin` includes the grouped-menu assertion; the constants check fails when a literal is added; the component check fails on hand-written markup; tests pass at 1280 and 390.
- Commit: `test(admin): rename steps and add drift checks`

**8.3 Update docs/style/ui-ux.md** · Model: Haiku

- Goal: Replace the admin-site part with the "Admin UI/UX rules" section of this plan (rules 1-48). Add "Single source of truth" (constants.rs vs tokens.css), remove "No charts by default", and update the "Admin pages" part for groups, components.html, components.js and constants.json. Keep the popup section untouched.
- Files: docs/style/ui-ux.md
- Checks: `scripts/check-docs.sh` passes; rules 1-48 appear once each; popup section unchanged in the diff; no "No charts by default" text.
- Commit: `docs(style): update the admin UI/UX rules`

**8.4 Turn audit-shots into a page screenshot script** · Model: Haiku

- Goal: Replace `audit-shots.js` with a script that visits every page in PAGES and the reference at 1280 and 390 and saves screenshots; the same script produces the images for docs.
- Files: tests/e2e/audit-shots.js (renamed to page-shots.js), package.json, docs/reference/admin-ui/components.md
- Checks: one 1280 and one 390 image per page in the output folder; script exits non-zero when a page fails to load; docs images regenerate from it; tests pass at 1280 and 390.
- Commit: `test(admin): turn audit-shots into a page screenshot script`

## Appendix C: testing notes and open questions from the original plan

### Testing and verification

Every commit leaves `cargo test -p askbar-admin` and `npm run test:services` green. The browser suite runs at 1280 × 900 and 390 × 844 from step 1.7.

- **Selectors.** About 80 browser steps use element ids. A step that moves or renames an element updates those selectors in the same commit and keeps ids where it can.
- **Page frame test.** `every_page_renders_with_the_nav_marking_it_current` in `pages.rs` changes once, in step 2.2. It then checks:
  - one `aria-current` per page
  - a link to every page
  - the Audit or Settings parent marked on its sub-pages
- **Constants.** `constants.json` must list every constant (step 1.1). From step 8.2, the scripts and templates may not contain a literal for any value it defines.
- **Tokens.** `scripts/check-ui-tokens.sh` keeps both `tokens.css` copies identical and checks contrast in both themes (step 1.11).
- **Components.** A `pages.rs` test renders every macro (step 1.8). From step 8.2, a check fails when a page hand-writes markup a component covers.
- **Reference.** A Rust test fails when a `help()` topic is missing from the reference index (step 7.3). A script checks reference ids and links (step 7.1).
- **Accessibility.**
  - Automatic: an axe-core scan of every page at both widths (step 1.11).
  - Manual, once per phase: keyboard only, one screen-reader pass (VoiceOver or NVDA), 200 % text zoom at 1280 px, reduced motion on.
- **CSP.** No inline script or style. New static files (`components.js`, `constants.json`) get routes in `lib.rs`. The Trusted Types and hostile-text steps stay as they are.
- **Visual check.** `tests/e2e/page-shots.js` (`npm run shots`) screenshots every page at both widths (step 8.5) for each phase review against page 3. It fails on a page that does not load, scrolls sideways or has text under 11px, but it compares no images.


**Open questions:**

- [ ] Mirror `docs/reference` to the GitHub wiki (step 7.6)? The wiki is a separate repository that anyone with wiki access can edit and that isn't versioned with releases. The recommendation is to link to `docs/reference` on GitHub, and mirror only if the wiki is wanted as the public front door.
- [ ] Later: server push. One server event stream (Server-Sent Events) for alerts, new clients and service status, so updates arrive instantly; `ui.live` switches to it and polling stays as the fallback. This is server work, outside this redesign.
- [ ] Claude and ChatGPT sign-in may only return to a fixed `http://localhost` address even with a domain set. Test this in step 5.5; the paste box stays as the fallback.
