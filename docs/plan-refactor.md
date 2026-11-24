# Plan: simplify the code before track 1

Status: proposed, 2026-11-24. Track 0 in [plan-priorities.md](plan-priorities.md). Runs as [orchestration.md](orchestration.md) says. No behaviour changes: every API, payload, record, check and UI stays as it is.

## Why

A review at commit 4d4df58 found about 2,500 lines that can go without losing a feature. Most of them are copies: the same link client, boot code, test setup and checks, written again per service. Track 1 adds three services (egress proxy, `mcp-gateway`, `searxng`), two or three admin links, and new tests and checks. Each would copy the same code again.

Each stage lands before the first track 1 stage that edits the same code:

| Stage | Before | Why |
| --- | --- | --- |
| R1 | W1 (AV2.1–AV2.3) | AV2.1 adds a span kind and a migration in `crates/askbar-audit/src/lib.rs`, and AV2.2 and W5.3 edit `generation.rs` |
| R2 | W2 (P4.2) | P4.2 is the first new container, and four later steps edit `scripts/check-isolation.sh` |
| R3 | W3 (P5A.6) | `services/mcp-gateway` is "built on `askbar-service`" and would be the fourth copy of the boot code |
| R4 | W4.2 | W4.2 and W5.1 edit the admin links, and P4.4 and P5A.18/19 add new ones. Run R4 before W2 if P4.4 lands first |
| R5 | W5 | W5.3, W5.4 and W5.5 add server tests, the client toggle and e2e fixtures |
| RL | Track 3 C1.1 and C2.2 | Both edit code that is duplicated today |

## Rules for every step

- **No behaviour change.**
  - The same tests pass without edits, except where a step moves or deletes test-only code.
  - API responses, audit records and the chain hash stay identical.
  - `check-isolation.sh` reports the same checks.
- **Moved code keeps its error handling and validation.**
- **One concern per commit**, with the exact subject below. Scope names follow `AGENTS.md`. `askbar-audit` and `askbar-service` use the scope of the service the step is about.
- **Isolation checks.** If a step changes `check-isolation.sh`, save its `PASS`, `FAIL` and `SKIP` lines before and after (`grep -E '^(PASS|FAIL|SKIP)'`). After the change there must be no `FAIL`. Name every removed or merged check in the commit body.
- **Small fixes on the way.** The [fold-in list](#fold-in-list) holds small fixes. The orchestrator may add one as its own commit in a stage that already edits that file. Otherwise leave it.

## Owner gates

| Gate | Needed before | What |
| --- | --- | --- |
| G-R0 | R1.1 | 1.0 released, unless the owner starts track 0 earlier |
| G-R1 | R3.1 | Approve adding the tokio features `net` and `signal` to `askbar-service`. They are features of an existing dependency, so no new crate. |
| G-R2 | R3.6 | Decide on [plan-single-image.md](plan-single-image.md). If it is accepted, skip R3.6, because its Phase 1.2 replaces the per-service stages. |
| G-R3 | none | The duplicated Trivy step in `ci.yml` and `images.yml` (one composite action) is the owner's, since agents don't edit `.github/` |

## R1: audit crate and generation

Branch `refactor/audit-and-generation`. PR title `refactor(auditor): simplify the audit crate and generation code`.

- [x] **R1.1** `refactor(auditor): remove the unused span recorder from askbar-audit` · sonnet
  Files: `crates/askbar-audit/{src/lib.rs,Cargo.toml,tests/chain.rs,tests/access_logs.rs}`, `services/model-gateway/tests/api.rs`.
  Do: delete the unused code:
  - `Recorder::start_span`, `finish_span`, `payload`, `dropped`, `noop` and `send`
  - `Event::Start`, `Finish` and `Payload`, and the `dropped` counter
  - `Kind` and the `SpanStart`/`SpanFinish` builder methods
  - `Sealed`
  - the in-crate tests for these

  Tests that used the builders build struct literals instead. Drop the `askbar-service` dependency, which only `ids::new_id` used. Keep the writer thread, `append`, batching, `prune`, `restore` and `access_log`.
  Accept: no caller outside the crate's own tests used the removed items (check with `graphify_callers` or grep); `cargo test -p askbar-audit -p askbar-auditor -p askbar-model-gateway` passes.
- [x] **R1.2** `test(auditor): pin the audit chain hash` · haiku
  Files: `crates/askbar-audit/tests/chain.rs`.
  Do: build a fixed set of records with fixed ids and times, and assert their entry hashes equal hex literals taken from the current code.
  Accept: the test passes on the current code. It guards AV2.1's "old records verify unchanged".
- [x] **R1.3** `refactor(auditor): drop the cost hook from audit stats` · sonnet
  Files: `crates/askbar-audit/src/lib.rs` (`stats`, `CostFn`, `CostSum`), `services/auditor/src/lib.rs` (the `stats` handler), `services/admin/src/audit_read.rs` (read only).
  Do: remove the cost argument and `CostSum`. The auditor always passes `|_,_,_,_| None`, and the admin sets `cost` and `cost_unknown_requests` itself (`audit_read.rs:17-47`). First add a test that pins the admin's stats JSON for a fixed store, in this same commit.
  Accept: the admin's stats JSON is unchanged; `docs/openapi/audit.v1.yaml` still matches the auditor's response.
- [x] **R1.4** `refactor(auditor): group audit stats in SQL` · sonnet · after R1.3
  Files: `crates/askbar-audit/src/lib.rs` (`stats`, `utc_day`).
  Do: replace the three per-group accumulators (by provider, by day, by day and provider) with `GROUP BY` queries. Replace `utc_day` with `strftime('%Y-%m-%d', started_at, 'unixepoch')`.
  Accept: R1.3's test and the existing stats tests pass unchanged.
- [ ] **R1.5** `refactor(auditor): move askbar-audit unit tests to their own file` · haiku
  Files: `crates/askbar-audit/src/lib.rs`, new `crates/askbar-audit/src/tests.rs`.
  Do: move the `mod tests` block as it is to `src/tests.rs` and declare it with `#[cfg(test)] mod tests;`.
  Accept: the same test count passes.
- [ ] **R1.6** `refactor(server): one sqlite open and migrate helper` · sonnet
  Files: `services/server/src/{db,space}.rs`.
  Do: one `open_migrated(conn, migrations, app_id, upto)` replaces the copied `open`, `open_in_memory`, `init` (the pragmas) and `migrate` (the `user_version` and `application_id` checks and the migration loop). `upto` keeps the test hook `Db::migrate(upto)` needs. Use `auth::now` for the copied `now()`.
  Leave the admin and `askbar-audit` copies alone: sharing them would need `rusqlite` in `askbar-service`.
  Accept: the migration tests pass; no migration SQL changes.
- [ ] **R1.7** `refactor(server): one function interrupts a generation` · sonnet
  Files: `services/server/src/generation.rs` (`sweep_on_start`, `shutdown`), `services/server/src/space.rs` (`get_generation`, read only).
  Do: `sweep_on_start` and `shutdown` both save partial text, finalize as interrupted and mark the question failed. One `interrupt_row` does it for both. `shutdown` reads its row with `get_generation` instead of its own SQL. Keep each caller's way of finding the provider.
  Accept: the server's restart and shutdown tests pass unchanged.
- [ ] **R1.8** `refactor(server): map a generation outcome in one place` · sonnet · after R1.7
  Files: `services/server/src/generation.rs` (`Outcome`, `Finisher::finish`).
  Do: give `Outcome` one method returning the stored status, the error, the failed flag and the `Terminal` event, so each variant is spelled out once. Fold in `terminal_of` only if it maps the same way.
  Accept: the chat tests pass unchanged.
- [ ] **R1.9** `refactor(server): remove unused cancel and catalog code` · haiku
  Files: `services/server/src/{generation,gateway}.rs`.
  Do:
  - Delete `Live::set_cancel`, the `cancel` field, its `take()` in `cancel()` and the `oneshot` import.
  - Delete `Gateway::forget_catalog`.
  - Put `Live::delta_count` and `Generations::len`/`is_empty` behind `#[cfg(test)]`.

  Accept: no callers remain (grep); `cargo test -p askbar-server` passes.
- [ ] **R1.10** `refactor(model-gateway): remove stream_chat` · haiku
  Files: `services/model-gateway/src/llm.rs`, `services/model-gateway/tests/streaming.rs`, `docs/plan-canaries.md` (C1.2: `sent_chars` is in `stream_chat_report`).
  Do: the two test calls use `stream_chat_report(...).map(|r| r.outcome).map_err(|e| e.error)`.
  Accept: `cargo test -p askbar-model-gateway` passes; `scripts/check-docs.sh` passes.
- [ ] **R1.11** `refactor(proto): remove unused split_model_id` · haiku
  Files: `crates/askbar-proto/src/lib.rs` (the function and its test assertions).
  Accept: no callers remain (grep across `crates`, `services` and `clients`); `cargo test -p askbar-proto` passes.

## R2: isolation checks

Branch `refactor/check-isolation`. PR title `refactor(compose): shorten the isolation checks`. Every step runs `scripts/check-isolation.sh` (needs Docker) and follows the isolation-check rule above. Keep every check that probes live behaviour:
- the 401/403/404 matrices
- the route-out and reachability probes
- the leak greps
- the backup, certificate and cookie checks
- the CapEff checks
- the failure drills

- [ ] **R2.1** `refactor(compose): define docker helpers once in check-isolation` · haiku
  Files: `scripts/check-isolation.sh`.
  Do: define `members`, `nets_of` and `mounts_of` once near the top. `members` is defined at three places today, and the inspect pipeline appears about 15 times. Use `drill_wait_healthy` for the 30×2s wait loops. Inline `check` (used once), and drop the single-element loops (`for h in caddy`, and the loop near line 881).
- [ ] **R2.2** `refactor(compose): one http helper in check-isolation` · haiku · after R2.1
  Files: `scripts/check-isolation.sh`.
  Do: one `http NET URL [METHOD] [TOKEN] [JSON]` replaces `http_status`, `http_get`, `gw_call` and `al_call`. Pass tokens with `-e`, never inside the Python source.
- [ ] **R2.3** `refactor(compose): admin setup and cleanup helpers in check-isolation` · haiku · after R2.2
  Files: `scripts/check-isolation.sh`.
  Do: the provider, user and pairing setup Python is pasted 4 times; make it `admin_setup_provider`. The catalog cleanup is pasted twice; make it `admin_cleanup_provider`.
- [ ] **R2.4** `refactor(compose): check each service from one profile table` · sonnet · after R2.3
  Files: `scripts/check-isolation.sh`.
  Do: one `svc_profile` function and a table with one row per service replace the per-service blocks for the auditor, `model-gateway`, the alerter, the server and `cpa`. The function checks health, uid, CapEff 0, no CapAdd, networks, internal flags, members, mounts, foreign volumes, and file owners and modes. Keep each service's real values in its row.
  Accept: a new service needs one row, not a block.
- [ ] **R2.5** `refactor(compose): check declared networks and mounts from compose config` · sonnet · after R2.4
  Files: `scripts/check-isolation.sh`.
  Do: read `docker compose config --format json` once, with the script's existing `py` helper (no new tool). From it, assert:
  - each service's networks, and each network's `internal` flag and members
  - read-only mounts and ports
  - the absence of `docker.sock`
  - the hardening fields
  - the pinned subnet (the grep near line 130)

  Keep one live `docker inspect` spot check per service, because the config is what was declared, not what runs.

## R3: shared service code

Branch `refactor/service-boot`. PR title `refactor(model-gateway): share service boot and link code`.

- [ ] **R3.1** `refactor(model-gateway): move service boot helpers into askbar-service` · sonnet · gate G-R1
  Files: `crates/askbar-service/{Cargo.toml,src/lib.rs}`, new `crates/askbar-service/src/boot.rs`, `services/{model-gateway,alerter,auditor}/src/main.rs`.
  Do: one copy each of `die`, `listen_addr`, `healthy` (taking the health path) and `stop_signal`, used by all three services. Leave the admin and server alone unless they use the same code without changes.
  Accept: each service starts and stops as before (its tests and `scripts/smoke.sh` pass).
- [ ] **R3.2** `refactor(model-gateway): share link routing with the alerter` · sonnet · after R3.1
  Files: `crates/askbar-service/src/links.rs`, `services/model-gateway/src/lib.rs`, `services/alerter/src/lib.rs`.
  Do: move `full`, `links_for`, `token_dir`, `shell` and `routes` into `askbar-service::links`. Use `Links::named()` for single-link tables. `server_links`, `admin_links` and `auditor_links` stay as one-line wrappers.
  Accept: the hash file paths and link names are unchanged; the link tests of both services pass.
- [ ] **R3.3** `refactor(proto): one table of link names` · haiku · after R3.2
  Files: `crates/askbar-proto/src/lib.rs` (or a new `links.rs`), `services/bootstrap/src/links.rs`, `services/{model-gateway,alerter,auditor}/src/lib.rs`.
  Do: the link names as constants in `askbar-proto`. Bootstrap and the services use them, so P5A.9a adds a new link name in one place.
  Accept: bootstrap's test that every link is a row of the `architecture.md` table still passes.
- [ ] **R3.4** `refactor(auditor): share the append step of the append handlers` · sonnet
  Files: `services/auditor/src/lib.rs`, `crates/askbar-audit/src/lib.rs` (`is_empty_of_content`).
  Do:
  - One `append_created(&a, id, record)` for the six handlers that gate, append and reply `201` (`start_span` to `add_alert_delivery`).
  - One `is_empty_of_content` on a `Connection` replaces the auditor's inline copy. Restore still checks it.

  Accept: the auditor API tests pass unchanged; `gate()` still runs before every append.
- [ ] **R3.5** `refactor(auditor): remove a re-export, a single-use trait and an unused parameter` · haiku
  Files: `services/auditor/src/{lib,rules}.rs`.
  Do: drop the `rusqlite_conn` module, make `ClassName` a function, and remove `_now` from `Rules::seen` and its caller.
- [ ] **R3.6** `refactor(compose): one runtime base stage in the Dockerfile` · sonnet · gate G-R2
  Files: `Dockerfile`.
  Do: the auditor, `model-gateway`, alerter and server runtime stages share one base stage with `ARG`s for the binary name and uid. Keep each stage's uid, owned directories and volumes explicit.
  Accept: `scripts/check-isolation.sh` passes, including the uid and owner checks.

## R4: admin

Branch `refactor/admin-links-and-ui`. PR title `refactor(admin): share link code and settings markup`. UI steps run `cargo test -p askbar-admin` and `npm test`. Keep the exports `tests/e2e/admin.test.js` calls (`window.ui.rowConfirm`, `menu`, `lengthHint`, `confirm`, `window.askbarAudit.*`). Update the macro lists in `services/admin/src/pages.rs` and `docs/reference/admin-ui/components.md` when macros change.

- [ ] **R4.1** `refactor(admin): one token reader and env default for the links` · haiku
  Files: `services/admin/src/{server_link,alerter_link,gateway_link,auditor_link,actions,lib}.rs`, plus the other files that repeat the env-default chain (`cpa.rs`, `backup_schedule.rs`, `admin_api.rs`, `caddy.rs`).
  Do:
  - One `read_link_token(token_dir, link)` replaces the 6 copies of `read_token`, `read_restore_token` and `append_token`. Keep `server_link::read_token`, which `tests/actions.rs` calls.
  - One `env_or(key, default)` replaces the 13 copies of `env::var(..).ok().filter(..)`.
- [ ] **R4.2** `refactor(admin): one link client for the alerter and gateway links` · sonnet · after R4.1
  Files: `services/admin/src/{alerter_link,gateway_link}.rs`, new `services/admin/src/link_client.rs`.
  Do: one struct holds the HTTP client, base URL, token, online state, last error, dirty flag, token watch, restarts and API prefix, with `new`, `set_notifier`, `online`, `last_error`, `set_error`, `mark_dirty` and `request`. Both links embed it and keep their own `sync` and `run`.
  Accept: `tests/alerts.rs` and `tests/gateway.rs` pass unchanged.
- [ ] **R4.3** `refactor(admin): split admin_api.rs into modules` · haiku
  Files: `services/admin/src/admin_api.rs`, new `services/admin/src/admin_api/{settings,sealed}.rs`.
  Do: move the settings section and the sealed-content section without changes.
  Accept: the route table and the OpenAPI test are unchanged.
- [ ] **R4.4** `test(admin): share fake link servers` · haiku
  Files: `services/admin/tests/{common/mod.rs,alerts.rs,gateway.rs,backup_bundle.rs,restore.rs}`.
  Do: move `fake_status`, `fake_restart`, `fake_config`, `Fake` and `fake_alerter` into `tests/common`. Merge two copies only where their response bodies match.
- [ ] **R4.5** `refactor(ui): settings section and number field macros` · haiku
  Files: `services/admin/src/ui/templates/{components,backup,general,notifications,security,network}.html`, `services/admin/src/pages.rs`, `docs/reference/admin-ui/components.md`.
  Do:
  - A `settings_section` call-body macro replaces the about 25 copies of the `sec-sec`/`sec-lab`/`sec-fields` scaffold.
  - A `number_field` macro replaces the 9 number inputs with their "min to max unit" hint.
  - In `security.html` the body is a `<form class="sec-fields">`; keep it as one.

  Accept: the rendered pages are unchanged apart from whitespace; the browser tests pass.
- [ ] **R4.6** `refactor(ui): share audit page helpers and inline errors` · haiku
  Files: `services/admin/src/ui/{audit,audit_calls,app,components}.js`.
  Do:
  - Put `explain`, `banner`, `DAY`, `duration`, `number` and `providerLabel` on `ui` once.
  - `audit.js` uses `ui.rows` for its three hand-built tables.
  - The 22 hand-written error displays use `inlineError`, and the four `onError` lambdas become one.
- [ ] **R4.7** `refactor(ui): remove the unused button helper and ETag cache` · haiku
  Files: `services/admin/src/ui/{components,app}.js`.
  Do:
  - Delete `ui.button`, which has no callers.
  - Delete the `etag` option, the `etags` map and the 304 branch from `api()`, and `{ etag: true }` from its 6 callers. The admin API sends no ETag on these routes; static files keep theirs.
  - Give `api()` a raw-body option so `uploadBundle` stops repeating its error and 401 handling.

## R5: tests and client

Branch `refactor/tests-and-client`. PR title `refactor(client): share test setup and popup helpers`.

- [ ] **R5.1** `test(server): one mock model launcher and serve helper` · haiku
  Files: `services/server/src/testing.rs` (or `tests/common/mod.rs`), `services/server/tests/{chat,audit,load,failures,router,server_api,read_api,auth,snapshot_auth}.rs`.
  Do:
  - `MockLlm::start()` (with the port and request log) replaces 6 launchers.
  - `serve(router)` replaces 11 bind-and-spawn pairs.
  - The local `provider()` builders wrap `testing::provider`.
  - `hex` and `client` are defined once.
- [ ] **R5.2** `test(auditor): share test setup` · haiku
  Files: `services/auditor/tests/{common/mod.rs,api.rs,alerts.rs,latency.rs,ingest.rs}`.
  Do: move `token`, `hex`, `T` (with its `Drop`) and `start` to `tests/common`.
- [ ] **R5.3** `refactor(client): one invoke wrapper and one refresh in the popup` · haiku
  Files: `clients/desktop/ui/main.js`, `clients/desktop/tests/`.
  Do:
  - `call(cmd, args, {quiet})` replaces the 9 `try { await invoke } catch { showNotice }` blocks and the `.catch(() => {})` calls.
  - `refreshAll()` replaces the repeated refresh sequences.
  - Delete `state.ui`, `state.provider` and `msg.provider`, which are written but never read.
- [ ] **R5.4** `refactor(client): one turn starter and shared action buttons` · haiku · after R5.3
  Files: `clients/desktop/ui/main.js`.
  Do:
  - One function starts a turn (user row, streaming assistant row, `streamInto`) at its 3 call sites.
  - `addError`, `addUnsent` and `addWaiting` use `actionButton`.
  - `el.foot` loses the `hidden` attribute that is only ever set to `false`. Check the first paint.
- [ ] **R5.5** `refactor(client): match 404 by status and share the cancel slot` · sonnet
  Files: `clients/desktop/src-tauri/src/{main,remote}.rs`.
  Do:
  - One helper holds the cancel-slot pattern of `ask`, `resend` and `attach`.
  - `cancel_generation` and `delete_conversation` check the HTTP status, not the text `"404 Not Found."`.
  - The `lock` helper is defined once.
- [ ] **R5.6** `test(e2e): one HTTP stub helper for the Python fixtures` · haiku
  Files: `tests/e2e/{releases_mock,ntfy_sink,smtp_sink}.py`, new `tests/e2e/stub.py`.
  Do: one shared `BaseHTTPRequestHandler` setup. W5.5's fixture `searxng` and pages use it.
  Accept: `npm test` and `scripts/smoke.sh` pass.

## RL: before track 3

Branch `refactor/before-canaries`. PR title `refactor(proto): share message trimming and event decoding`.

- [ ] **RL.1** `refactor(proto): share normalize and last_turns` · haiku
  Files: `crates/askbar-proto/src/lib.rs`, `services/server/src/llm.rs`, `services/model-gateway/src/llm.rs`.
  Do: move the two functions and one copy of their tests to `askbar-proto`. Both services call it.
- [ ] **RL.2** `refactor(model-gateway): decode stream events in one place` · sonnet
  Files: `services/model-gateway/src/llm.rs` (`run_request`).
  Do: the two `match decode_event(...)` blocks become one helper, so C1.1 sets `finish_reason` once.

## Fold-in list

Small fixes, done only by a stage that already edits the file, each as its own commit:
- `hex()` copies (12): one helper, or `format!("{:x}", digest)`.
- A `LockExt::lock_ok()` for the 77 `.lock().unwrap_or_else(|e| e.into_inner())` calls in the admin.
- Admin UI:
  - duplicate CSS selectors in `style.css`
  - `table_head` column classes
  - merge `blocks.html` into `components.html`
  - an `id` argument for the `button` macro
  - `ui.confirm` versus `dialog_box`, and the key-revoke confirm
  - the hard-coded 1/7/30 range options
- Desktop: `problem_of` with a `#[serde(default)]` struct; the `String()` wrappers and `node._msg`; fewer `layout()` calls; `<dialog>` for the panels.
- Optional and not planned: table-drive `chain.rs` (after R1.2's golden test), and fold `askbar-audit` into the auditor.
