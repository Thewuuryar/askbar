# Plan: from 0.5.0 to v1

Status: tracker for the v1 build order. Decisions live in the [ADRs](adr/README.md) and [architecture.md](architecture.md); this file only orders the work. Tick a step by deleting it once its PR merges, and delete the file at 1.0.

## Context

`main` (0.5.0) is three programs plus a controller: `askbar-server` (client API, server API, provider keys under a master key, the router, and the sealed audit store in `audit.db`), `askbar-admin` (site at `/admin`, API at `/admin/api/v1`, first run on `127.0.0.1:8081`), `askbar-controller` (Docker socket), `askbar-proto`, and the Tauri client (`src-tauri/`, `ui/`). The accepted ADRs (0001–0032) and `docs/architecture.md` describe a different v1: no controller, a `bootstrap` container with per-link tokens, an `auditor`, a `model-gateway` that alone holds keys and opens spans, an `alerter`, per-space SQLite files, encrypted per-service backups, hardened containers, and `/api/{name}/v1` paths everywhere.

This plan gets there in small PRs. Rules for every PR:

- **Working at every step.** After each PR, `docker compose up -d --build` on Docker Desktop for Mac gives a stack that a freshly built desktop client can pair with and chat through (the "Mac check" below). No PR leaves it broken. The one place where three PRs must land together is marked.
- One concern per PR, tests and docs in the same PR, `npm test` and `scripts/smoke.sh` pass; `scripts/check-isolation.sh` too when compose, networks, volumes or Caddy change (AGENTS.md).
- Pre-1.0, no data migration ([ADR 0018](adr/0018-releases-and-migrations.md), owner confirmed): a PR that changes storage says "wipe volumes and re-pair". Data volume names never change; a volume that stops being used is left in compose with a comment until 1.0.
- Owner decisions taken for this plan: the client pins Caddy's self-signed certificate via the pairing key (needs ADR 0034); iOS is the last, deferrable phase.
- Branches `<type>/<kebab>`, Conventional Commit subjects. PRs that touch `.github/workflows/` are listed but need the owner's go-ahead first.

Size: S ≈ 1–2 h, M ≈ 3–5 h, L ≈ 6–10 h (owner budget ~100 h to March 2027, [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md); the defer order in [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md) applies if it slips).

### The Mac check (run for every PR)

```bash
docker compose --profile dev down --remove-orphans   # --profile dev also removes mock-llm, which would otherwise keep a deleted network
docker compose --profile dev up -d --build
```
1. Open the admin at `https://localhost/admin` (or the Mac's LAN address) and accept the certificate warning, sign in, add the mock-llm provider, create a pairing key.
2. `cd clients/desktop && npm install && npm run dev`, pair from the tray, send a message, see a streamed reply, close and reopen the window mid-reply.
3. `scripts/smoke.sh` passes.

---

### Component ADRs (owner rule, 2026-11-07)

A component is anything the stack or the repository ships as its own unit: a container (service), a library crate, or a client. Each component has an ADR of its own that says what it is, what it holds, what it can reach, its volumes, networks and links, and any hardening exception; it links to the decision ADRs instead of repeating them. Under [ADR 0001](adr/0001-decision-records-and-change-rules.md):

- **New component:** its ADR is proposed in its own `docs(adr):` PR, and the owner accepts it before the PR that creates the component merges. Numbers are taken in order when the ADR is written, never reserved.
- **Changed component:** its ADR is amended in place, in the PR with the code (ADR 0001 rule 13).
- **Retired component:** its ADR's status becomes `Retired`, with a `Retired:` line giving the date and the reason (ADR 0001 rule 11). The index in [adr/README.md](adr/README.md) follows.
- **Existing components** get retroactive ADRs once phase 1 is done (step 1.14), all Proposed, so the owner reviews what is there before phases 2 to 4 add more.

Steps that create or retire a component say which ADR they need. Estimate: about 1 h per component ADR.

## Phase 0 — Restructure (no behaviour change)

Target layout:

```
askbar/
  Cargo.toml  Cargo.lock      services and crates only
  compose.yaml                v1 stack; cpa under a profile
  Dockerfile                  one target per service
  crates/       askbar-proto/  askbar-audit/  askbar-service/
  services/     server/  admin/  auditor/  model-gateway/  alerter/  bootstrap/
  clients/
    desktop/                  own workspace: src-tauri/, ui/, tests/
    ios/                      Swift, generated from docs/openapi/client.v1.yaml (phase 8)
  deploy/       caddy/  cpa/
  tests/e2e/                  mock_llm.py, admin.test.js, the 3-user load target
  scripts/
  docs/                       adr/, openapi/, style/
```

**0.1 `refactor: move services and crates to the v1 layout`** (M)
- ADR 0033 "Repository layout" (records the layout above; architecture.md "Considered and not changed" updated: `src-tauri/` and `ui/` keep their names under `clients/desktop/`).
- `git mv`: `askbar-proto` → `crates/askbar-proto`; `askbar-server` → `services/server`; `askbar-admin` → `services/admin`; `askbar-controller` → `services/controller` (temporary, deleted in 1.4); `caddy/` → `deploy/caddy/`; `cpa/` → `deploy/cpa/`; `tests/mock_llm.py`, `mock_llm_test.py`, `admin.test.js`, `audit-shots.js` → `tests/e2e/`.
- `Dockerfile.caddy` becomes the `caddy` target of `Dockerfile` (one target per service); compose `build.target` updated.
- Package names and binary names unchanged. Update `Cargo.toml` members, `Dockerfile` COPY lines, `compose.yaml`, `scripts/*.sh`, `package.json`, `include_str!` paths (admin UI, Caddyfile test), README "Layout", CONTRIBUTING, AGENTS commands, `check-docs.sh` link targets.
- `.github/workflows/build.yml` paths: listed, owner approval needed.
- Verify: `cargo test --workspace`, `npm test`, smoke, isolation (Dockerfile changed), Mac check.

**0.2 `refactor(client): give the desktop client its own workspace`** (M)
- `src-tauri/`, `ui/` → `clients/desktop/src-tauri`, `clients/desktop/ui`; `tests/ui.test.js`, `markdown.test.js`, `mock-tauri.js`, `make_icons.py` → `clients/desktop/tests/`.
- `clients/desktop/Cargo.toml` (`[workspace] members = ["src-tauri"]`, own `Cargo.lock`), path dep `../../../crates/askbar-proto`. `clients/desktop/package.json` holds `@tauri-apps/cli` and the `dev`/`build`/`test` scripts. Root `Cargo.toml` lists services and crates only; the Dockerfile `sed` hack goes.
- Root `npm test` runs root checks, `cargo test --workspace`, `tests/e2e` and `npm --prefix clients/desktop test` (which runs `cargo test` there).
- Verify: both workspaces build and test; `npm run dev` from `clients/desktop` on the Mac.

New crates `crates/askbar-service` and `crates/askbar-audit` are created in the first PR that needs each (1.3 and 2.3), not empty now.

## Phase 1 — Bring the existing components to the target

Order change (owner approved 2026-11-07): the remaining steps keep their numbers as IDs but run in this order.

| # | Step | Size | Why here |
| --- | --- | --- | --- |
| — | 1.13 backlog | see below | After 1.11, which reshapes the conversation routes the S items touch. The owner chose to build every item but summarizing before phase 2 (2026-11-08), one pull request at a time, because they touch `generation.rs` and the popup, which 2.4 also changes. Summarizing waits for 3.1 (its model call goes through `model-gateway`). |
| — | 1.16 a page and a URL for every admin section | M–L | After 1.11, or when scheduled. Deferrable, cut with the 1.13 backlog. |
| — | 1.12 shared spaces | M | Deferred to after phase 7 ([ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md) defer order). |

Bootstrap tokens (2.1) stay in phase 2: nothing in phase 1 needs per-link tokens beyond the one admin→server token that exists.

### Budget

Midpoints S = 1.5 h, M = 4 h, L = 8 h. Required work left (excluding 1.12, 1.13 and phase 8):

| Phase | Hours |
| --- | --- |
| 1 (1.1–1.5, 1.8–1.11) | 32.5 |
| Component ADRs (1.14, and one each for bootstrap, `askbar-audit`, auditor, `model-gateway`, alerter) | ≈ 9 |
| 1.15 `askbar-cpa` supervisor and the name check removal (added 2026-11-07) | ≈ 3.5 |
| 2 | 35 |
| 3 | 21.5 |
| 4 | 9.5 |
| 5 | 11 |
| 6 | 17.5 |
| 7 | 12.5 |
| **Total** | **≈ 152** |

That is about 52 h over the ~100 h budget before any backlog item (the component ADRs and the `cpa` supervisor, added 2026-11-07, are about 12 h of it). Proposed cuts, first to last: phase 8 (iOS, memory, mismatch alerts); 1.13 summarizing (L, ~8 h); the rest of 1.13 (~12 h) and 1.16 (~6 h); 6.4 multiple destinations (one daily destination stays: it is the ship criterion for data loss in [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md)); 1.12 shared spaces; 5.4 passkey (optional, after 1.0); 4.3 release check. Those cuts save about 8 h of required work (5.4, 4.3, part of 6.4), leaving ≈ 142 h. The owner kept the budget unchanged (2026-11-07); the gap is recorded here so cuts can be made in this order if hours run out.

**1.12 `feat: shared spaces`** (M) — [ADR 0009](adr/0009-spaces-and-storage.md) (deferrable, last in [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md) order)
- Admin create/rename/delete (with confirmation), add/remove members with the "new members see history" notice; client space picker.

**1.13 Backlog: failed turns and long conversations** (owner request, 2026-11-07; estimate before starting). Since `fix(server): leave unanswered questions out of later prompts`, a question that got no reply is shown but not sent to the model again.

Order (owner, 2026-11-08), before phase 2: message actions on hover with the model and provider per reply; then failed questions stay marked with Edit and Try again; then copy the whole conversation; then the history length limit; then newest at the top or bottom. Summarizing is deferred to after 3.1. Anything a reopened conversation must show is stored on the message: finished generations are pruned.
- **Edit and Try again on a failed line** (S): the failed question in the window gets **Edit** (puts its text back in the box) and **Try again** (resends it), for every failure kind, not only timeouts.
- **History length limit** (S): an admin setting for how much history is sent with each question. Today the whole conversation is sent every turn, with no limit.
- **Summarize when the context would overflow** (L): when a conversation is expected to exceed the model's context length, the server summarizes (compacts) the older part and sends the summary instead; the client shows "Summarizing the conversation…" with a progress bar while it runs. Needs a design note first: which model summarizes, where the summary is stored, and how the extra model call is audited (it goes through `model-gateway` like any other, phase 3).
- **Failed questions stay marked** (S/M): a question that got no reply is shown with a coloured background (a `--danger` tint that passes contrast in both themes) and its error text, live and after reopening the chat. Today the error line exists only in the live window; a reopened conversation shows just the unanswered questions. Needs the client API to say which questions failed and why: an additive field on each message in the conversation detail (for example `failed: {code, message}` from the turn's generation, linked by `user_message_id`), the OpenAPI file and wire contract, and the popup rendering. Pairs with Edit and Try again.
- **Newest at the top or bottom** (S): show the conversation newest-first, with the input box following it, or oldest-first as now. The admin sets the default (a new display-defaults field, server-wide and per user, edited in the admin site); a client can override it locally (`client.yaml` `ui`), as with the other display defaults.
- **Copy the whole conversation** (S): a copy button on each conversation in the history panel, next to its delete button, that copies every question and reply, in order, as Markdown with who said what.
- **Message actions on hover** (S/M): under each question and each reply, a copy icon (replacing today's Copy text button) and the date and time it was sent, both shown when the pointer is over that message or it has keyboard focus. Each message keeps the padding for them at all times, so nothing moves when they appear. Needs each message's time in the popup (`created_at` is stored; check it reaches the window).
- **Model and provider per reply** (S/M; owner request, 2026-11-08): the person can change providers between messages, so each reply records the provider and model that wrote it, a copy taken when it is stored, and a small icon next to its date and time has a tooltip naming both.
- **Built so (2026-11-08), message actions and model and provider per reply:**
  - **Storage:** space migration 2 adds `provider_id`, `provider_name` and `model` to `messages`, and `provider_name` and `model` to `generations`. The generation keeps the copy while it runs, so a reply written by the start-up sweep or at shutdown is labelled too. Questions, and replies stored before the migration, have none.
  - **API (additive):** `Message.provider` (`{id, name, model}`) in the conversation detail, and `model` on the `provider` stream event; `MIN_CLIENT` unchanged.
  - **Popup:** a meta row under every message: the date and time (a `<time>` in the person's locale), a chip on replies whose `title` and `aria-label` read `Model: … · Provider: …`, and the copy button as an icon. The row's height is always kept and only its opacity changes; its controls stay reachable by Tab, which shows it. The gap between messages shrank to keep the thread about as tall as before.
  - **Not as the plan first said:** there is no provider fallback (ADR 0014), so the chip always names the provider that was asked for.
- **Built so (2026-11-10), failed questions stay marked, with Edit and Try again:**
  - **Storage:** space migration 3 adds `failed_code` and `failed_message` to `messages`, set on the question when its turn ends as `provider_error`, `timed_out`, `kill_switch`, `internal` or `interrupted`, whether or not part of a reply was kept. A Stop is not a failure. The message is always the server's own sentence; for a provider error it is "The provider could not answer.", never the provider's text.
  - **API (additive):** `Message.failed` (`{code, message}`) in the conversation detail; `MIN_CLIENT` unchanged.
  - **Popup:** a question that got no reply has a `--danger-soft` tint (a new token; `check-ui-tokens.sh` checks `--fg` on it); its error line follows it, or follows the partial reply if one was kept, live and after reopening. The line offers **Try again** (asks the same text again as a new question below; the failed one stays marked) and **Edit** (puts the text back in the box), for every failure kind. Try again after a timeout now shows the question again, as the server stores it.
  - **The live error keeps the provider's text** (owner, 2026-11-10): the `error` event, and `generations.error` until pruned, still carry the provider's own words; only the copy on the question uses the server's sentence.
- **Built so (2026-11-10), history length limit:** a new admin setting on the Settings page, "History sent with each question": how many earlier questions, each with its reply, go with a new one (0 to 1000; 0, the default, sends the whole conversation, as before). It reaches the server as the snapshot's `history_turns` (additive; an older snapshot means 0) and is read when a generation starts. The server cuts the history after leaving out unanswered questions and before the attempt, so the sealed audit record of what was sent is what was sent.
- **Built so (2026-11-10), copy the whole conversation:** client only. Each row in the history panel has a copy icon before Delete, shown like Delete on hover or keyboard focus. It reads the conversation (`load_conversation`) without opening it and copies Markdown: `# <title>`, then for each message `**You**` or `**Assistant**`, its date and time, and on a reply `<model> via <provider>`, then the text; a failed question is followed by `> Failed: <why>`.
- **Built so (2026-11-10), newest at the top or bottom:** a new display default, `newest_first` (false, oldest first, as before), set server-wide on the admin's Settings page and per user through the display-defaults API, and overridable in `client.yaml` (`ui.newest_first`). Newest first keeps the input bar at the top (owner's choice, 2026-11-10) and puts the latest turn just under it; each question stays above its reply and error line. The popup reverses whole turns with each row's flex `order`, so the rows stay in time order for copying and the shortcuts, and it keeps the top of the thread in view instead of the bottom. An error line now has a 12 px gap under it, like the meta row under other messages.

**1.16 `feat(admin): a page and a URL for every admin section, from one template`** (M–L ≈ 6 h; owner request, 2026-11-07; deferrable with the 1.13 backlog, so outside the required total). Today the admin site is one page whose tabs don't change the address, so a section can't be linked, bookmarked or reached with Back.
- **Pages:** `/admin/providers/`, `/admin/services/`, `/admin/clients/`, `/admin/users/` (from 1.9), `/admin/audit/`, `/admin/backup/`, `/admin/network/`, `/admin/security/`, `/admin/settings/`. `/admin` and `/admin/` redirect to the first. Unknown `/admin/...` paths stay 404.
- **One list of pages:** `services/admin/src/pages.rs` holds `PAGES: [(slug, label, template)]`. The routes in `lib.rs` (replacing the single `/admin/` route), the nav, and the route-table test all come from it.
- **One template (Askama):** `ui/templates/base.html` holds the frame, written once:
  - head (`tokens.css`, `style.css`, `app.js` with `defer`);
  - header with sign-out;
  - nav from `PAGES`, with `aria-current="page"` on the current page;
  - page heading with a one-line hint, and the content block;
  - the sign-in, first-run and offline-banner screens.

  Each page is `ui/templates/<slug>.html`, which extends `base.html` and fills `title`, `hint` and `content`. Templates compile into the binary and render once at start, so serving a page costs what serving a file does now.
- **Building blocks** (`ui/templates/blocks.html`, Askama macros), used the same way on every page: a card, a list with its empty-state line, a form with its error line, a section hint, and the confirm wording. `docs/style/ui-ux.md` gets an "Admin pages" section: a new page is one `PAGES` entry plus one template using these blocks.
- **Behaviour:**
  - Nav entries become plain `<a href>`, so Back, Forward and bookmarks work without routing code.
  - `app.js` runs only the current page's loader (the `tabs[].load` table stays; `renderNav` and `openTab` go).
  - `audit.js` loads on the Audit page only. Audit deep links use the query string (`/admin/audit/?view=trace&id=<trace id>`): it is read on load and kept current with `history.replaceState`.
  - The first-run "next steps" links become page links.
- **Unchanged:** the API; the session cookie (`Path=/api/admin`); the CSP (`script-src 'self'`, no inline script); `textContent`-only rendering in scripts. Askama escapes by default, and templates hold only static text and the page list.
- **Weight and speed:** browsers get plain pre-rendered HTML, with no framework or client-side rendering.
  - **Today:** every visit loads about 92 KB (the 24 KB page with all sections, 35 KB `app.js`, 23 KB `audit.js`, 10 KB CSS).
  - **After:** a page carries only its own section.
  - **Caching:** `security_headers` (`web.rs`) sends `no-store` on everything today. Static files get `no-cache` with an `ETag` instead, so a click costs one small page plus 304s. Pages and the API stay `no-store`.
  - **Devices:** this keeps it fast on an old laptop or a Raspberry Pi's browser. An ESP32 can neither host Askbar nor run a browser, whatever the design.
- **New dependency:** `askama` (MIT OR Apache-2.0, inside the license allowlist, so a reason in the PR is enough, [ADR 0026](adr/0026-dependency-license-check.md)). The reason: compile-time checked templates, escaping by default, and no template files to read at run time.
- **Tests:**
  - **Browser:** each page opens by URL, the nav marks it current, Back returns to the previous page, `/admin/audit/?view=trace&id=…` opens directly, every page has the same header, nav and banners, and a static file answers 304 on a second load.
  - **Rust:** every `PAGES` entry has a route and a nav link, and an unknown `/admin/x/` is 404.
- **Docs:** the guide's admin site steps ([guide.md](guide.md#server)) and `docs/style/ui-ux.md`.
- **Built so (2026-11-08):** as above, with these differences. **Eight pages, not nine:** users are managed on the Clients page, where step 1.9 put them, so there is no `/admin/users/`, and it is a 404. **Paths:** the templates are in `services/admin/src/ui/templates/`, next to the scripts, with `services/admin/askama.toml` pointing Askama there; Askama 0.16. **Building blocks:** `blocks.html` has `error_line` and `hint`, the two pieces of markup that repeat verbatim (28 uses); a card, a list and a form are written out, because today's sections differ in shape, and `docs/style/ui-ux.md` says to add a block on the third use. **Hints:** five sections had no hint under their heading; each got one line. **Scripts:** `app.js` wires an element only when it is on the page. The browser tests wait for a page to load after each nav click (`openPage`), and three new steps cover the URLs, Back, the Audit deep link and the 304s.

## Phase 2 — Add bootstrap and the auditor

**Order (owner, 2026-11-10):** 2.3, 2.1, 2.2, 2.4a (the auditor standing alone, with nothing sending to it), the `model-gateway` component ADR, 3.1, then 2.5 to 2.9, then 3.2 to 3.5. Reason: only `model-gateway` may open model-call spans ([ADR 0010](adr/0010-auditor.md) rule 2, [ADR 0051](adr/0051-auditor.md)), so the server switches to the auditor in the same step that adds the gateway.

**2.1 `feat(bootstrap): create per-link tokens on first start`** (M) — [ADR 0004](adr/0004-bootstrap-and-per-link-tokens.md), 0038
- Before the code: a component ADR for `bootstrap` (its volumes, the links it writes, no network, run once), proposed in its own PR and accepted.
- `services/bootstrap`: no network, mounts credential volumes only, writes each token to the caller's volume and its hash to the receiver's, exits; `depends_on: condition: service_completed_successfully`. Host rotation: `docker compose run --rm bootstrap rotate [link]`, then restart from the admin.
- Replace the admin-minted server token (`server-token` volume keeps its name).
- Learned in 1.10: every container now runs with `cap_drop: [ALL]`, so root can't write or read another uid's files. `bootstrap` must create each token and hash with the owner and mode its reader needs (or run as that reader's uid), and the isolation script's hardening section must list `bootstrap`. Links table from architecture.md is the source of truth; unit test the link list.
- **Built so (2026-11-10):** as above, with these differences. **Takeover:** with every capability dropped, root can neither read nor stat a directory that belongs to another uid, so for each credential volume `bootstrap` chowns it to itself (mode 0700), checks and writes, then gives every file and directory to its reader (tokens 0400, hashes 0444, directories 0500). A run that stops half way leaves a volume taken over, and the next run takes it again. A link is complete only when its token and its hash both exist; otherwise both are made again, because `bootstrap` cannot read a token it does not own to derive the missing hash. **Randomness:** `/dev/urandom`, so the crate adds only `base64` and `sha2`. **Volumes and layout:** `admin-token` is new and `server-token` keeps its name; both have `call/` and `accept/`. **Uids** are in a table checked against the Dockerfile (admin 10001, server 10002). **Server:** reads `/server-token/accept/admin-server.sha256` and refuses to start without it, with a message that says to run `docker compose up -d`. **Admin:** reads `/admin-token/call/admin-server.token`, deletes its old `server.token` on start, and mounts no server volume. **Compose:** `bootstrap` has `user: "0"`, `cap_add: [CHOWN]`, `network_mode: none`; `server` and `admin` wait for it. **Development runs:** `ASKBAR_BOOTSTRAP_ROOT` gives the volumes under a directory to the current user, which is how `scripts/smoke.sh` and the browser tests run it. **Commits:** the compose service was added in one commit under a profile and wired in the next, so no commit leaves a stack that cannot start. **Isolation script:** section 3d3 checks `bootstrap` (no network, exit 0, only the two credential volumes, `CHOWN` only) and each file's owner and mode, and that each service mounts only its own credential volume, read-only.

**2.2 `feat: alert when a previously accepted token is rejected`** (S) — [ADR 0024](adr/0024-rejected-token-alert.md)
- Caller records the failure; receiver counts repeated invalid tokens. Delivered as a log + admin banner now, wired to the alerter in 4.2.
- **Built so (2026-11-10):** as above, with these differences. **Caller:** a `401` is the only answer that means the token was not accepted (any other status came after the check). The admin flags it on the first `401` of an episode, logs one line (the link, whether the server accepted the token before, the rotate command; never the token), and clears it at the next answer that is not a `401`. A token never accepted since the admin started is flagged too, with its own wording. The status endpoint gains `token_rejected`; the banner gives the rotate command and `docker compose restart server admin` (the admin can't restart the server over a link the server refuses), and replaces the "server offline" banner while it shows. **Receiver:** `askbar-service` counts tokens that match no link per listener (a missing token, or a valid one on the wrong route, is not counted) in a sliding ten-minute window, and logs once when the count reaches five, then not again until it has dropped below five, so one fault is one line. The line names the links as `bootstrap` does (`admin-server`) and the count. The count is kept in memory (at most 1,024 timestamps); it does not survive a restart. **Alerts:** the alerter does not exist yet (4.2); until then these are the log line and the banner.

**2.3 `refactor: move the audit store into crates/askbar-audit`** (S)
- Before the code: a component ADR for the `askbar-audit` crate (what it holds, who links it: server now, auditor in 2.4), proposed in its own PR and accepted.
- Pure move of `services/server/src/audit.rs` into a library; server keeps using it in-process. `seal.rs` stays in the server, and the span record types move to `askbar-proto` in 2.4, when they become wire types ([ADR 0052](adr/0052-askbar-audit-crate.md)).
- **Built so (2026-11-10):** as above. `new_id` moved first, in its own commit, to `askbar_service::ids`; the server re-exports it as `ids`, so its callers are unchanged. The store is `crates/askbar-audit/src/lib.rs`, moved unchanged apart from paths and two edits: `Sealed` is defined there (`seal.rs` re-exports it), and `stats` takes a cost callback (`CostFn`) instead of calling `cost`; `PriceOf` and `cost_of` moved to the server's `cost.rs`, and `server_api.rs` passes the callback. The server keeps `pub use askbar_audit as audit`. The crate's dependencies are the ones the server already used: `rusqlite`, `tokio`, `serde_json`, `base64`, `thiserror`, `tracing`, plus `askbar-service`. The Dockerfile copies its manifest for the dependency-cache layer. `docs/architecture.md` lists no crates one by one and [ADR 0033](adr/0033-repository-layout.md) already names `askbar-audit`, so neither changed.

**2.4 `feat(auditor): run the audit store as its own container`** (L) — [ADR 0010](adr/0010-auditor.md), 0025
- Before the code: a component ADR for the `auditor` (its volume `auditor-data`, its internal network, its links and tokens, no outbound), proposed in its own PR and accepted. The `server` component ADR (0039) is amended in place in the same PR: it no longer mounts `audit-data` ([ADR 0001](adr/0001-decision-records-and-change-rules.md) rule 13).
- `services/auditor`: `/api/audit/v1` append (service and admin append tokens), read and retention (admin read token, floor 7 days, default 30), boot-ID handling that ends earlier open spans as interrupted, group commit. Internal-only network, no outbound.
- Server appends over HTTP with a bounded retry queue for finish/payload records; the auditor mounts a new volume, `auditor-data`; the server's `audit-data` stays declared and unmounted until 1.0 ([ADR 0051](adr/0051-auditor.md)). Server answers `503 audit_backlog` when the auditor is unreachable.
- Failure tests: auditor down, disk full, slow ack.
- **Built so, 2.4a (2026-11-10): the auditor standing alone.** The plan's PR D, as the owner's order has it: nothing sends the auditor records until step 3.1, and the server keeps recording to its own file until then. **Store** (`askbar-audit`): migration 3 adds `service` and `boot_id` to spans, `admin_actions` (never changed or deleted), `prunes` (a record written in the same transaction as each delete, never changed or deleted) and `settings`; `Recorder::append` answers after the commit and is idempotent; the writer commits what is queued in one transaction (group commit); `backlog()` reports the queued count and the age of the oldest. **API** (`/api/audit/v1`, `docs/openapi/audit.v1.yaml`): four links, each with its own token hash in `auditor-token/accept/<link>.sha256` and its own routes (the server's opens `gate` spans only; `model-gateway`'s opens any kind; `admin-auditor-append` records admin actions; `admin-auditor-read` reads and sets retention), a hello that ends a service's earlier unfinished spans as interrupted, retention (default 30 days, never below 7, pruned daily, each prune recorded), token counts by provider and model with no prices. **Limits:** 1,000 queued records or an oldest record waiting over 5 s refuses every append; below 256 MiB free a new span is refused (`503 audit_backlog`, `Retry-After`, nothing recorded). Free space comes from `df`: the standard library has no call for it and `unsafe` is denied, and `df` needs no dependency. **Bootstrap:** `admin-auditor-append` and `admin-auditor-read` links, and the volume `auditor-token`; the Dockerfile's `auditor` target is in that commit, not the compose one, because bootstrap's uid test reads the Dockerfile, and sits before the server's so a plain `docker build .` still builds the server. **Compose:** `auditor` (uid 10003, the shared hardening, no exception), `auditor-data`, and the network `audit-admin` (internal, admin and auditor only); the admin joins it. **Isolation:** section 3d4. Not done: "no unsealed content reaches its store" (nothing sends content until 3.1), and the acknowledgment latency is measured and printed, not gated (7.2). Acknowledgment p95 is about 1.5 ms here. Left for later steps: `audit-server`, `audit-model-gateway` and `audit-alerter` networks (their callers), hash chain (2.6), admin action recording (2.7).

**2.5 `refactor(admin): read the audit trail from the auditor`** (M)
- Admin Audit tab reads via the admin read token; remove `/api/server/v1/audit/*` and its OpenAPI paths.

**2.6 `feat(auditor): hash chain and chain-head check`** (M) — [ADR 0025](adr/0025-audit-record-reliability.md), 0016
- Each record carries the previous hash; admin stores the head periodically and flags rollback/edit; tamper test.

**2.7 `feat(admin): record every admin action and refuse it if the record fails`** (M) — [ADR 0010](adr/0010-auditor.md)
- All categories in [ADR 0010](adr/0010-auditor.md); services record the config version they applied ([ADR 0028](adr/0028-service-boundaries-and-config-delivery.md) rule 5).

**2.8 `feat(admin): sealed-content sessions`** (M) — [ADR 0011](adr/0011-sealed-content-access.md), 0030
- Fresh sign-in + passphrase; key in memory only; 15 min idle / 1 h total (configurable to 12 h); ends on Seal, sign-out, restart; start/end/each item viewed recorded; refused if not recorded. Key-ID registry with created/retired dates.

**2.9 `feat(caddy): send access logs to the auditor`** (M) — if the design adds a log-forwarding sidecar, that is a new component and needs its own ADR first, under the rule above. Needs a short design note (Caddy's `net` log writer carries no token: likely a tiny ingest listener on the auditor reachable only from Caddy's network, or a log-forwarding sidecar). Propose as an [ADR 0016](adr/0016-risk-register.md) implementation note before building.

## Phase 3 — Add model-gateway

**3.1 `feat(model-gateway): move provider calls and keys out of the server`** (L) — [ADR 0014](adr/0014-model-gateway-and-providers.md), 0031 (wipe and re-enter keys)
- Before the code: a component ADR for `model-gateway` (keys in memory only, outbound access, its networks and links), proposed in its own PR and accepted. The `server` component ADR changes (no keys, no outbound): amend or supersede it as above.
- `services/model-gateway`: `/api/model-gateway/v1` streaming chat endpoint; keys pushed by the admin in `PUT /config`, held in memory only; opens the span in the auditor and waits for the ack before calling a provider; metadata-only independent records.
- Server calls the gateway with user, space, turn; adds finish/payload records to the returned span ID. Server drops `secrets.rs`, `llm.rs` provider code, `master.key` (stop mounting `askbar-secrets`, keep it declared).
- Networks: server moves to internal networks only (Caddy↔server, server↔gateway, server↔auditor); gateway gets outbound; `mock-llm` joins the gateway's network in the dev profile. Isolation: server has no route out.

**3.2 `feat(model-gateway): budgets and kill switch`** (M) — [ADR 0014](adr/0014-model-gateway-and-providers.md), 0003
- Gateway copy authoritative; server copy for early refusal and usage; per-user pause flag (emergency stop level).

**3.3 `feat: approved model catalog with one default`** (M) — [ADR 0014](adr/0014-model-gateway-and-providers.md)
- `provider/model` IDs; admin picks from each provider's `/v1/models` and must set a default; gateway serves the catalog to the server; client picks a model per chat (replaces the provider picker); conversations store their model; gateway and server refuse anything else.

**3.4 `feat(model-gateway): streaming provider test with time to first token`** (S) — [ADR 0014](adr/0014-model-gateway-and-providers.md)
- Fixed prompt; pass/fail, TTFT, redacted details view; metadata-only audit record.

**3.5 `feat(cpa): connect cpa to model-gateway automatically`** (M) — [ADR 0014](adr/0014-model-gateway-and-providers.md)
- CLIProxyAPI reads its config only at start (found in 1.4); after 1.15 its supervisor restarts it when the admin writes the config, so no host step is needed.
- `cpa` on a gateway-only network; first start writes its config and adds it as a provider; paste-URL login: matches address and state, 10 min, single use, never logged. Test both providers against the pinned image.

## Phase 4 — Add the alerter

**4.1 `feat(alerter): deliver alerts through ntfy or email`** (M) — [ADR 0006](adr/0006-v1-outbound-access-and-alerts.md), 0038
- Before the code: a component ADR for the `alerter` (its settings volume, outbound for delivery only, its links), proposed in its own PR and accepted.
- `services/alerter`: `/api/alerter/v1`, delivery settings on its own volume, auditor → alerter events, outbound only for delivery; admin configures delivery and sends a test alert.

**4.2 `feat: alert rules`** (M) — [ADR 0010](adr/0010-auditor.md), 0011, 0016, 0024, 0005
- Unseal and timeout change, retention change, backup key change, rejected/invalid tokens, silent appending component, auditor disk early warning, restart loops, budget thresholds; repeat limiting.

**4.3 `feat(alerter): check for new releases`** (S) — [ADR 0017](adr/0017-dependencies-and-patches.md)
- Release source polled without user data; admin reads the result and shows it.

## Phase 5 — Remaining defend controls and client policy

**5.1 `feat: privacy notice and AI warning`** (S) — [ADR 0012](adr/0012-consent-notices-and-deletion.md)
- Notice at pairing and when relevant settings change (default on, editable); AI warning above the chat box (default on, editable); deleting a user deletes the personal space file.

**5.2 `test: malicious markdown payload library`** (M) — [ADR 0016](adr/0016-risk-register.md)
- Block remote images/media, safe links with destination shown, Trusted Types where supported; payload library run against the popup and admin views; browser test proves no network request or script.

**5.3 `feat(caddy): HTTP-01 certificates`** (S) — [ADR 0015](adr/0015-certificates.md)

**5.4 `feat(admin): optional passkey with recovery codes`** (M) — [ADR 0007](adr/0007-first-run-and-admin-sign-in.md) (needs a domain; can slip after 1.0 since "optional")

## Phase 6 — Backups

**6.1 `feat(admin): backup passphrase and age key pair`** (S) — [ADR 0013](adr/0013-backups.md)
- Wizard step, ≥20 chars, private key encrypted with it, public key pushed to each service; key change audited and alerted.

**6.2 `feat: encrypted exports from each service`** (L)
- `POST /exports` on server, auditor (read-only), model-gateway, alerter, admin; a `cpa` export wrapper; admin bundles with a manifest (hashes, release version), browser download. Test: admin cannot decrypt an export.

**6.3 `feat(admin): restore into an empty install`** (M)
- Version check before the passphrase; refuse newer releases; CI restore test with a throwaway key and no audit passphrase.

**6.4 `feat(admin): scheduled backups, destinations and retention`** (M)
- `/backup` volume and SMB/NFS volume in compose; writable check, same-disk warning; schedule; 5 files / 30 days, newest kept; missed-backup alert; 90-day drill reminder. Test an unavailable share does not keep the admin down.

## Phase 7 — Ship gates

**7.1 `ci: pull-request workflow`** (M, owner approval) — rustfmt (`style/rustfmt` first), clippy, `npm test`, smoke, `check-isolation.sh`, `cargo deny` + npm license check ([ADR 0026](adr/0026-dependency-license-check.md)), gitleaks, Dependabot/Renovate, scheduled CVE scan of crates and images ([ADR 0017](adr/0017-dependencies-and-patches.md)).

**7.2 `test(e2e): three simultaneous users and the latency budget`** (M) — [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md), 0031
- `tests/e2e` load target: 3 concurrent replies against the mock; p95 < 200 ms before first token excluding the provider; auditor ack p95 < 20 ms.

**7.3 `test: reboot, restart and failure drills`** (S) — reboot comes back with no commands; auditor and admin failure tests.

**7.4 `docs: v1 documentation pass`** (S) — README describes v1, architecture diagram matches shipped services, component matrix, failure table, network map ([ADR 0003](adr/0003-v1-service-layout.md)), AGPL note.

**7.5 `chore: release 1.0`** — tags, GHCR images pinned by digest, staging compose project ([ADR 0018](adr/0018-releases-and-migrations.md)). Person-run items from the [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md) checklist: real certificate, restore drill under 30 min, client on macOS/Windows/Linux, risk sign-off.

## Phase 8 — Deferrable (cut first if hours run out)

- **8.1 Memory** per space with creator recorded ([ADR 0009](adr/0009-spaces-and-storage.md)).
- **8.2 `clients/ios`**: Swift client generated from `docs/openapi/client.v1.yaml`, same error-code mapping; first decide swift-format vs SwiftLint (contributor plan #14). Before the code: a component ADR for the iOS client, proposed and accepted.
- Mismatch alerts ([ADR 0010](adr/0010-auditor.md)).

---
