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
docker compose --profile dev up -d --build
```
1. Open the admin at `https://localhost/admin` (or the Mac's LAN address) and accept the certificate warning, sign in, add the mock-llm provider, create a pairing key.
2. `cd clients/desktop && npm install && npm run dev`, pair from the tray, send a message, see a streamed reply, close and reopen the window mid-reply.
3. `scripts/smoke.sh` passes.

---

### Component ADRs (owner rule, 2026-11-07)

A component is anything the stack or the repository ships as its own unit: a container (service), a library crate, or a client. Each component has an ADR of its own that says what it is, what it holds, what it can reach, its volumes, networks and links, and any hardening exception; it links to the decision ADRs instead of repeating them. Under [ADR 0001](adr/0001-decision-records-and-change-rules.md) and [ADR 0027](adr/0027-adr-format-and-numbering.md):

- **New component:** its ADR is proposed in its own `docs(adr):` PR, and the owner accepts it before the PR that creates the component merges. Numbers are taken in order when the ADR is written, never reserved.
- **Retired component:** if its ADR is still Proposed, amend it (ADR 0001 rule 3): say it is retired, when and why. If it is Accepted and on `main`, write a new ADR that supersedes it and change only the old ADR's status line to `Superseded by NNNN` (ADR 0001 rule 2). The index in [adr/README.md](adr/README.md) follows either way.
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
| 5 | 1.3 `crates/askbar-service` | M | Base for 1.4 and for every service in phases 2–4. |
| 6 | 1.4 self-restart, controller removed | M | Needs 1.3's health and token check. Does not wait for 2.1: restart is authenticated with the existing admin→server token, and 2.1 later changes only where that token comes from, not the endpoint. Doing it now removes the only Docker socket mount. The restart record is a log line until 2.7 sends it to the auditor. |
| 7 | 1.8 first run and forced password change | M | Independent of storage; before 1.9 so the wizard's last step can create the first user. |
| 8 | 1.9 users and pairing keys in the admin | M | 1.9 and 1.11 both change storage and pairing; next to each other they cost one "wipe volumes and re-pair" instead of two. |
| 9 | 1.11 spaces, one SQLite file each | L | As above. Wipe and re-pair once, at the end of this step. |
| 10 | 1.14 retroactive component ADRs | M | Closes phase 1: the components that exist then get their ADRs before phase 2 adds new ones. |
| — | 1.13 backlog | see below | After 1.11, which reshapes the conversation routes the S items touch. Summarizing waits for 3.1 (its model call goes through `model-gateway`). |
| — | 1.12 shared spaces | M | Deferred to after phase 7 ([ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md) defer order). |

Bootstrap tokens (2.1) stay in phase 2: nothing in phase 1 needs per-link tokens beyond the one admin→server token that exists.

### Budget

Midpoints S = 1.5 h, M = 4 h, L = 8 h. Required work left (excluding 1.12, 1.13 and phase 8):

| Phase | Hours |
| --- | --- |
| 1 (1.1–1.5, 1.8–1.11) | 32.5 |
| Component ADRs (1.14, and one each for bootstrap, `askbar-audit`, auditor, `model-gateway`, alerter) | ≈ 9 |
| 2 | 35 |
| 3 | 21.5 |
| 4 | 9.5 |
| 5 | 11 |
| 6 | 17.5 |
| 7 | 12.5 |
| **Total** | **≈ 149** |

That is about 49 h over the ~100 h budget before any backlog item (the component ADRs, added 2026-11-07, are about 9 h of it). Proposed cuts, first to last: phase 8 (iOS, memory, mismatch alerts); 1.13 summarizing (L, ~8 h); the rest of 1.13 (~12 h); 6.4 multiple destinations (one daily destination stays: it is the ship criterion for data loss in [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md)); 1.12 shared spaces; 5.4 passkey (optional, after 1.0); 4.3 release check. Those cuts save about 8 h of required work (5.4, 4.3, part of 6.4), leaving ≈ 139 h. The owner kept the budget unchanged (2026-11-07); the gap is recorded here so cuts can be made in this order if hours run out.

**1.3 `feat(server): answer 503 not_configured until the admin pushes config`** (M) — [ADR 0028](adr/0028-service-boundaries-and-config-delivery.md)
- Component ADR: [0036 `askbar-service` crate](adr/0036-askbar-service-crate.md) (Proposed in the same PR, as the rule was set while the PR was open; to be accepted before it merges).
- New `crates/askbar-service`: `/health`, `/status`, `PUT /config` scaffolding, config version, boot ID, per-link bearer check (hash file → caller → allowed routes), `not_configured` error code.
- The server already answers `GET /status` (`config_version`, `boot_id`) and `PUT /config` since 1.2; 1.3 moves them into the crate.
- Server uses it; admin polls `GET /status` every 10–15 s and re-pushes when unconfigured or stale; a refused change stays "nothing applied" (already true).

**1.4 `feat!: replace the controller with self-restart`** (M) — [ADR 0005](adr/0005-no-controller-and-self-restart.md)
- Retires the controller. It has no component ADR to amend or supersede: accepted [ADR 0005](adr/0005-no-controller-and-self-restart.md) already decides its removal, so the PR cites it and adds no ADR. (Owner to confirm.)
- `POST /restart` on server and admin (auth, audit record (local until 3.x), cooldown, shutdown limit), internal watchdog for failed health; admin restarts Caddy by config reload; test `cpa` reload and restart it by config rewrite, adding a supervisor only if needed.
- Build `POST /restart` on `crates/askbar-service` (the restart route can join `routes`, and its link rules are `links::Allow`). After a restart the admin already pushes again within one status check (12 s), from the new boot ID (1.3).
- Delete `services/controller`, the `control` network, `control-token` volume reference, `docker.sock` mount, `CONTROL_V1`, `control.v1.yaml`, isolation section 3e (replace with "no service mounts docker.sock").
- Test: scripted restart from the admin returns under 1 minute ([ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md)).

**1.8 `feat(admin): first run with published credentials and a forced password change`** (M) — [ADR 0007](adr/0007-first-run-and-admin-sign-in.md)
- Initial `admin`/documented password; every route except password change refuses until changed; reject the default and username-equal passwords; host reset command (`docker compose run --rm admin reset`) recorded. Login lockout, rate limits, identical errors (Defend list, [ADR 0016](adr/0016-risk-register.md)).
- Wizard: password → audit passphrase → (backup passphrase added in 6.1) → optional URL/DNS → providers → user and pairing key.

**1.9 `feat: users and pairing keys created in the admin`** (M) — [ADR 0008](adr/0008-users-clients-and-client-access.md), 0022
- Admin keeps users; `POST /api/admin/v1/users`, `POST /api/admin/v1/users/{id}/pairing-keys`; server receives users in config; pairing registers a client for an existing user. `POST /api/client/v1/me/pairing-keys` for extra clients; client UI "Pair another app". Rename "pairing string/code" to "pairing key" in UI and docs.

**1.11 `feat: spaces with one SQLite file each`** (L) — [ADR 0009](adr/0009-spaces-and-storage.md) (wipe and re-pair)
- Server: `control.db` (users, clients, pairing hashes, spaces, memberships, budget copy) and one file per space under `/data/spaces/`; a `SpaceStore` handed to handlers only after a membership check; migrations run on every file. Drop the unused `projects` table.
- Client API: `GET /spaces`, `/spaces/{space_id}/conversations[/{id}]`, `/spaces/{space_id}/search`, `/spaces/{space_id}/chat`; flat routes removed. Client uses the personal space by default. `MIN_CLIENT` bump, `client.v1.yaml`, `wire-contract.md`.
- Test: a bug-shaped query in one space file cannot return another space's rows; cross-user access refused.

**1.14 `docs(adr): retroactive ADRs for the existing components`** (M, after 1.11; owner rule 2026-11-07)
- One Proposed ADR per component that exists when phase 1 is done, each in its own commit, all in one PR, and each listed in the ADR index: `server` (`services/server`), `admin` (`services/admin`), `caddy` (`deploy/caddy` and the Dockerfile target), `cpa` (the pinned CLIProxyAPI image and `deploy/cpa`), `mock-llm` (dev profile and tests only), the desktop client (`clients/desktop`), and the `askbar-proto` crate. (`askbar-service` already has [0036](adr/0036-askbar-service-crate.md); the controller is retired in 1.4.)
- Each describes the component as it is on `main` then, with "Not recorded" where the reasoning is unknown ([ADR 0027](adr/0027-adr-format-and-numbering.md)), and links the decision ADRs ([0003](adr/0003-v1-service-layout.md), [0028](adr/0028-service-boundaries-and-config-delivery.md), [0032](adr/0032-container-hardening.md) and the rest) instead of repeating them. Where the code and an accepted ADR disagree, the component ADR says so and the PR lists it.

**1.12 `feat: shared spaces`** (M) — [ADR 0009](adr/0009-spaces-and-storage.md) (deferrable, last in [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md) order)
- Admin create/rename/delete (with confirmation), add/remove members with the "new members see history" notice; client space picker.

**1.13 Backlog: failed turns and long conversations** (owner request, 2026-11-07; estimate before starting). Since `fix(server): leave unanswered questions out of later prompts`, a question that got no reply is shown but not sent to the model again.
- **Edit and Try again on a failed line** (S): the failed question in the window gets **Edit** (puts its text back in the box) and **Try again** (resends it), for every failure kind, not only timeouts.
- **History length limit** (S): an admin setting for how much history is sent with each question. Today the whole conversation is sent every turn, with no limit.
- **Summarize when the context would overflow** (L): when a conversation is expected to exceed the model's context length, the server summarizes (compacts) the older part and sends the summary instead; the client shows "Summarizing the conversation…" with a progress bar while it runs. Needs a design note first: which model summarizes, where the summary is stored, and how the extra model call is audited (it goes through `model-gateway` like any other, phase 3).
- **Failed questions stay marked** (S/M): a question that got no reply is shown with a coloured background (a `--danger` tint that passes contrast in both themes) and its error text, live and after reopening the chat. Today the error line exists only in the live window; a reopened conversation shows just the unanswered questions. Needs the client API to say which questions failed and why: an additive field on each message in the conversation detail (for example `failed: {code, message}` from the turn's generation, linked by `user_message_id`), the OpenAPI file and wire contract, and the popup rendering. Pairs with Edit and Try again.
- **Newest at the top or bottom** (S): show the conversation newest-first, with the input box following it, or oldest-first as now. The admin sets the default (a new display-defaults field, server-wide and per user, edited in the admin site); a client can override it locally (`client.yaml` `ui`), as with the other display defaults.
- **Copy the whole conversation** (S): a copy button on each conversation in the history panel, next to its delete button, that copies every question and reply, in order, as Markdown with who said what.
- **Message actions on hover** (S/M): under each question and each reply, a copy icon (replacing today's Copy text button) and the date and time it was sent, both shown when the pointer is over that message or it has keyboard focus. Each message keeps the padding for them at all times, so nothing moves when they appear. Needs each message's time in the popup (`created_at` is stored; check it reaches the window).

## Phase 2 — Add bootstrap and the auditor

**2.1 `feat(bootstrap): create per-link tokens on first start`** (M) — [ADR 0004](adr/0004-bootstrap-and-per-link-tokens.md), 0021
- Before the code: a component ADR for `bootstrap` (its volumes, the links it writes, no network, run once), proposed in its own PR and accepted.
- `services/bootstrap`: no network, mounts credential volumes only, writes each token to the caller's volume and its hash to the receiver's, exits; `depends_on: condition: service_completed_successfully`. Host rotation: `docker compose run --rm bootstrap rotate [link]`, then restart from the admin.
- Replace the admin-minted server token (`server-token` volume keeps its name).
- Learned in 1.10: every container now runs with `cap_drop: [ALL]`, so root can't write or read another uid's files. `bootstrap` must create each token and hash with the owner and mode its reader needs (or run as that reader's uid), and the isolation script's hardening section must list `bootstrap`. Links table from architecture.md is the source of truth; unit test the link list.

**2.2 `feat: alert when a previously accepted token is rejected`** (S) — [ADR 0024](adr/0024-rejected-token-alert.md)
- Caller records the failure; receiver counts repeated invalid tokens. Delivered as a log + admin banner now, wired to the alerter in 4.2.

**2.3 `refactor: move the audit store into crates/askbar-audit`** (S)
- Before the code: a component ADR for the `askbar-audit` crate (what it holds, who links it: server now, auditor in 2.4), proposed in its own PR and accepted.
- Pure move of `services/server/src/audit.rs` and `seal.rs` into a library; server keeps using it in-process.

**2.4 `feat(auditor): run the audit store as its own container`** (L) — [ADR 0010](adr/0010-auditor.md), 0025
- Before the code: a component ADR for the `auditor` (its volume `audit-data`, its internal network, its links and tokens, no outbound), proposed in its own PR and accepted. The `server` component ADR from 1.14 changes too (it no longer mounts `audit-data`): amend it if still Proposed, or supersede it if Accepted.
- `services/auditor`: `/api/audit/v1` append (service and admin append tokens), read and retention (admin read token, floor 7 days, default 30), boot-ID handling that ends earlier open spans as interrupted, group commit. Internal-only network, no outbound.
- Server appends over HTTP with a bounded retry queue for finish/payload records; the `audit-data` volume is mounted by the auditor instead of the server (same name). Server answers `503 audit_backlog` when the auditor is unreachable.
- Failure tests: auditor down, disk full, slow ack.

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
- `cpa` on a gateway-only network; first start writes its config and adds it as a provider; paste-URL login: matches address and state, 10 min, single use, never logged. Test both providers against the pinned image.

## Phase 4 — Add the alerter

**4.1 `feat(alerter): deliver alerts through ntfy or email`** (M) — [ADR 0006](adr/0006-v1-outbound-access-and-alerts.md), 0021
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
