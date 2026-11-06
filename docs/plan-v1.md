# Plan: from 0.5.0 to v1

Status: tracker for the v1 build order. Decisions live in the [ADRs](adr/README.md) and [architecture.md](architecture.md); this file only orders the work. Tick a step by deleting it once its PR merges, and delete the file at 1.0.

## Context

`main` (0.5.0) is three programs plus a controller: `askbar-server` (client API, server API, provider keys under a master key, the router with fallback, and the sealed audit store in `audit.db`), `askbar-admin` (site at `/admin`, API at `/admin/api/v1`, first run on `127.0.0.1:8081`), `askbar-controller` (Docker socket), `askbar-proto`, and the Tauri client (`src-tauri/`, `ui/`). The accepted ADRs (0001–0032) and `docs/architecture.md` describe a different v1: no controller, a `bootstrap` container with per-link tokens, an `auditor`, a `model-gateway` that alone holds keys and opens spans, an `alerter`, per-space SQLite files, encrypted per-service backups, hardened containers, and `/api/{name}/v1` paths everywhere.

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

**1.1 `refactor(admin)!: move the admin API to /api/admin/v1`** (M) — [ADR 0022](adr/0022-api-path-conventions.md), 0029
- `paths::ADMIN_V1 = "/api/admin/v1"`; admin router, site `fetch` calls, `admin.v1.yaml`, tests, smoke, isolation script, Caddy route generator (`services/admin/src/caddy.rs`) and allowlist cover `/admin` and `/api/admin`; everything else stays 404.
- Cookie: `__Secure-` prefix, `Secure`, `Path=/api/admin` (`web.rs:65`). Add `Sec-Fetch-Site` refusal for cross-site changes next to the CSRF header check (`web.rs:101`).
- Test each [ADR 0029](adr/0029-shared-origin-protections.md) rule (cookie attributes, CSRF, Sec-Fetch-Site, CSP, `nosniff` on the client API).

**1.2 `refactor(server)!: follow the path conventions on the server API`** (S) — [ADR 0022](adr/0022-api-path-conventions.md)
- `/models/list` → `GET /models`; `PUT /snapshot` → `PUT /config`; add `GET /status` (config version, boot ID); `/backup` → `POST /exports` placeholder kept unencrypted until phase 6. Admin `/services/{name}/{action}` → literal `POST /services/{name}/restart` (start/stop dropped with the controller in 1.4; keep the controller call behind it until then).

**1.3 `feat(server): answer 503 not_configured until the admin pushes config`** (M) — [ADR 0028](adr/0028-service-boundaries-and-config-delivery.md)
- New `crates/askbar-service`: `/health`, `/status`, `PUT /config` scaffolding, config version, boot ID, per-link bearer check (hash file → caller → allowed routes), `not_configured` error code.
- Server uses it; admin polls `GET /status` every 10–15 s and re-pushes when unconfigured or stale; a refused change stays "nothing applied" (already true).

**1.4 `feat!: replace the controller with self-restart`** (M) — [ADR 0005](adr/0005-no-controller-and-self-restart.md)
- `POST /restart` on server and admin (auth, audit record (local until 3.x), cooldown, shutdown limit), internal watchdog for failed health; admin restarts Caddy by config reload; test `cpa` reload and restart it by config rewrite, adding a supervisor only if needed.
- Delete `services/controller`, the `control` network, `control-token` volume reference, `docker.sock` mount, `CONTROL_V1`, `control.v1.yaml`, isolation section 3e (replace with "no service mounts docker.sock").
- Test: scripted restart from the admin returns under 1 minute ([ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md)).

**1.5 `refactor(server): remove automatic provider fallback`** (S) — [ADR 0014](adr/0014-model-gateway-and-providers.md)
- `router.rs` tries only the chosen provider (retries kept); drop `fallback_order`, `allow_as_fallback`, the `provider` "answered by" event usage in the client UI. Contract change noted in the commit.

Order change: 1.6 and 1.7 were done first, right after phase 0. Before them a Docker stack had no client route at all without a host name and a Cloudflare token, so the Mac check could not pass; the plan had assumed the 8081 page served clients, and it served the admin only.

**1.6 `feat(caddy)!: self-signed HTTPS from first boot on the private address`** (M) — [ADR 0007](adr/0007-first-run-and-admin-sign-in.md), 0015
- Caddy serves `/admin`, `/api/admin`, `/api/client` on 443 with a certificate the admin makes for the install (Caddy's internal CA would need on-demand TLS, since Caddy doesn't know the host's address), handed out for any name, 80 redirects; private-address `remote_ip` allowlist from boot (real source, not forwarded headers). Remove the `admin-setup` network and the `127.0.0.1:8081` port.
- Isolation script: allowlist rules, no 8081, published ports 80/443 only.

**1.7 `feat(client): pin the self-signed certificate from the pairing key`** (M) — ADR 0034 (new, proposed)
- Pairing key carries the certificate's SHA-256 (the admin made it, so it reads it from its own file). Client pins it with a custom rustls verifier while there is no host name; normal validation once a real cert is issued; `allow_insecure` stays dev-only. The pin is an optional field, so older pairing keys still decode.
- *1.6 and 1.7 must land together for the Mac check to pass over HTTPS* (the two-PR window).

**1.8 `feat(admin): first run with published credentials and a forced password change`** (M) — [ADR 0007](adr/0007-first-run-and-admin-sign-in.md)
- Initial `admin`/documented password; every route except password change refuses until changed; reject the default and username-equal passwords; host reset command (`docker compose run --rm admin reset`) recorded. Login lockout, rate limits, identical errors (Defend list, [ADR 0016](adr/0016-risk-register.md)).
- Wizard: password → audit passphrase → (backup passphrase added in 6.1) → optional URL/DNS → providers → user and pairing key.

**1.9 `feat: users and pairing keys created in the admin`** (M) — [ADR 0008](adr/0008-users-clients-and-client-access.md), 0022
- Admin keeps users; `POST /api/admin/v1/users`, `POST /api/admin/v1/users/{id}/pairing-keys`; server receives users in config; pairing registers a client for an existing user. `POST /api/client/v1/me/pairing-keys` for extra clients; client UI "Pair another app". Rename "pairing string/code" to "pairing key" in UI and docs.

**1.10 `feat(compose): harden every container`** (S) — [ADR 0032](adr/0032-container-hardening.md)
- `x-hardening` block (`read_only`, `cap_drop: [ALL]`, `no-new-privileges`, `pids_limit`, `mem_limit`, `tmpfs: /tmp`); Caddy keeps `NET_BIND_SERVICE`; `cpa` exceptions documented after testing; `host.docker.internal` moves to `compose.dev.yaml`. Isolation script checks each setting.

**1.11 `feat: spaces with one SQLite file each`** (L) — [ADR 0009](adr/0009-spaces-and-storage.md) (wipe and re-pair)
- Server: `control.db` (users, clients, pairing hashes, spaces, memberships, budget copy) and one file per space under `/data/spaces/`; a `SpaceStore` handed to handlers only after a membership check; migrations run on every file. Drop the unused `projects` table.
- Client API: `GET /spaces`, `/spaces/{space_id}/conversations[/{id}]`, `/spaces/{space_id}/search`, `/spaces/{space_id}/chat`; flat routes removed. Client uses the personal space by default. `MIN_CLIENT` bump, `client.v1.yaml`, `wire-contract.md`.
- Test: a bug-shaped query in one space file cannot return another space's rows; cross-user access refused.

**1.12 `feat: shared spaces`** (M) — [ADR 0009](adr/0009-spaces-and-storage.md) (deferrable, last in [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md) order)
- Admin create/rename/delete (with confirmation), add/remove members with the "new members see history" notice; client space picker.

## Phase 2 — Add bootstrap and the auditor

**2.1 `feat(bootstrap): create per-link tokens on first start`** (M) — [ADR 0004](adr/0004-bootstrap-and-per-link-tokens.md), 0021
- `services/bootstrap`: no network, mounts credential volumes only, writes each token to the caller's volume and its hash to the receiver's, exits; `depends_on: condition: service_completed_successfully`. Host rotation: `docker compose run --rm bootstrap rotate [link]`, then restart from the admin.
- Replace the admin-minted server token (`server-token` volume keeps its name). Links table from architecture.md is the source of truth; unit test the link list.

**2.2 `feat: alert when a previously accepted token is rejected`** (S) — [ADR 0024](adr/0024-rejected-token-alert.md)
- Caller records the failure; receiver counts repeated invalid tokens. Delivered as a log + admin banner now, wired to the alerter in 4.2.

**2.3 `refactor: move the audit store into crates/askbar-audit`** (S)
- Pure move of `services/server/src/audit.rs` and `seal.rs` into a library; server keeps using it in-process.

**2.4 `feat(auditor): run the audit store as its own container`** (L) — [ADR 0010](adr/0010-auditor.md), 0025
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

**2.9 `feat(caddy): send access logs to the auditor`** (M) — needs a short design note (Caddy's `net` log writer carries no token: likely a tiny ingest listener on the auditor reachable only from Caddy's network, or a log-forwarding sidecar). Propose as an [ADR 0016](adr/0016-risk-register.md) implementation note before building.

## Phase 3 — Add model-gateway

**3.1 `feat(model-gateway): move provider calls and keys out of the server`** (L) — [ADR 0014](adr/0014-model-gateway-and-providers.md), 0031 (wipe and re-enter keys)
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
- **8.2 `clients/ios`**: Swift client generated from `docs/openapi/client.v1.yaml`, same error-code mapping; first decide swift-format vs SwiftLint (contributor plan #14).
- Mismatch alerts ([ADR 0010](adr/0010-auditor.md)).

---
