# Refactor log

## S4: test baseline (972c1f6)

| Suite | Count |
| --- | --- |
| client (`src-tauri` unit) | 14 passed, 1 ignored |
| `askbar-core` unit | 25 |
| `askbar-core` tests/streaming.rs | 8 |
| `askbar-proto` unit | 5 |
| `askbar-server` unit (lib; includes 1 `import.rs` test) | 16 |
| `askbar-server` tests/admin_auth.rs | 11 |
| tests/admin_backup.rs | 2 |
| tests/admin_devices.rs | 3 |
| tests/admin_network.rs | 6 |
| tests/admin_providers.rs | 5 |
| tests/admin_settings.rs | 3 |
| tests/admin_setup.rs | 5 |
| tests/auth.rs | 7 |
| tests/caddy_apply.rs | 6 |
| tests/chat.rs | 4 |
| tests/listeners.rs | 1 |
| tests/read_api.rs | 4 |
| tests/rotation.rs | 8 |
| tests/setup_mode.rs | 1 |
| tests/version_gate.rs | 4 |
| Node `markdown` | 19 |
| Node `ui` | 39 |
| Node `admin` | 23 |

Rust total: 138 passed (1 ignored).

Notes:
- The task list names a standalone `import.rs` test file; at this commit there is none. The `import` tests are 1 unit test inside `askbar-server/src/import.rs`.
- The remote tag pushes (S2) were skipped on the user's instruction; tags exist locally only. The remote `server-client` branch was not deleted (S3), so `c75fb37` stays reachable.
- Tauri build needs GTK/WebKit dev packages (`libgtk-3-dev`, `libwebkit2gtk-4.1-dev`, etc.) in this container.

## Checkpoint R1 (T1)

- `cargo build --workspace`, `cargo test --workspace`: 137 passed, 0 failed (baseline 138; the only difference is the 1 `import` unit test, removed with the feature).
- `cargo clippy --workspace --all-targets`: no errors; 6 style warnings that predate R1 (e.g. `useless_format`, `needless_question_mark`). No `dead_code` warnings remain.
- Node: markdown 19, ui 39, admin 23, all passing, equal to baseline.
- R1.1: the task names tests for `create-pair-code`; none existed, so none were removed.
- R1.3: clippy reported one dead item, `TokenStore::clear`; it was removed with its `allow(dead_code)`. The `allow(dead_code)` at `backend.rs:70` is left for R2.4, which deletes that code.

## Order change: R2.3 pulled the id change forward

R2.3 as written (proto `Conversation`/`Message` in the client) cannot pass the `ui` tests while Tauri commands and the UI use numeric ids, which the list assigns to R2.11. By agreement, R2.3 switched the `Backend` trait, Tauri commands, `ui/main.js`, `ui.test.js` and `mock-tauri.js` to string ids and deleted `Remote.ids`. R2.11 keeps what is left (stream events were already string ids; check for leftovers when reached).
R2.3 also asked to drop the `config_path`, `db_path` and `Config` imports from `main.rs`; they were still used by the standalone branch, so they went in R2.4.
R2.2: core's `Config` lost its `ui` field; the clamp and defaults tests moved to `client_config.rs`.
R2.4: `backend.rs` became `types.rs`; `Backend`/`Local` and the `async-trait` dependency are gone. With no `client.yaml` the client now uses the default config, so the window opens and reports "`server_url` is not set" until the user pairs from the tray. The tray's "Open config file" now opens `client.yaml`, and the standalone welcome notice is gone.
R2.5: the existing offline check was part of another test; a dedicated test was added.

## Checkpoint R2a (T1)

- Rust: 139 passed, 0 failed (baseline 138: -1 `import` test, +1 `ui_defaults`, +1 R2.5 test).
- Clippy: no errors; the same 5 pre-existing style warnings.
- Node: markdown 19, ui 39, admin 23, all passing.

## R2.6 notes

- `core` folded into `askbar-server` (`llm.rs`, `db.rs`; `Provider`, `ProviderKind` and `DEFAULT_SYSTEM_PROMPT` now live in `llm.rs`). `config.rs`, `default_config.yaml` and the `core` crate are gone.
- Tests removed with `config.rs` (they tested YAML `Config` loading, which is deleted): `bundled_default_is_valid_and_complete`, `minimal_yaml_gets_defaults`, `empty_file_falls_back_to_builtin_providers`, `parse_errors_name_the_file`, `unknown_active_provider_lists_options`, `load_or_create_writes_default_once`. `key_resolution_prefers_inline_then_env` moved to `llm.rs`; `base_url_defaults_per_kind` was added to keep the `effective_base_url` check. The streaming and provider tests keep their counts; they build their `Provider` directly instead of from `Config::builtin()`.
- Rust total after R2.6: 134 passed, 0 failed (139 - 6 removed + 1 added).
- One `cargo test --workspace` run during R2.6 reported 1 failure (the output was cut off, so the test is unknown); 7 later full runs, including `--no-fail-fast`, were clean. Not reproduced; watch for flakiness.
- `docker build` of the server image: passed. This container's build needs the egress proxy CA, so it was built from a scratch copy of the Dockerfile that only adds the CA (`CARGO_HTTP_CAINFO`); the repo's Dockerfile is unchanged apart from dropping `COPY core`. The image's `healthcheck` command ran.

## R2.8 notes

- One migration now holds the final chat schema (`users`, `devices` with `revoked_at`, `conversations`, `messages`, `messages_fts` with sync triggers, `user_prefs`; reserved `memory` (append-only via triggers), `projects`, `token_usage`; interim `providers`, `settings`, `pair_codes`, `display_defaults`). `conversations.project_id` is nullable.
- The version check refuses a database whose `user_version` is newer than the migrations, and any non-empty database whose `application_id` is not Askbar's (so a v0.2.0 database, `user_version` 2, is refused even after later migrations are appended). `Db::open` returns an error and the server exits.
- There is no upgrade path from v0.2.0 databases: the old schema (integer ids plus `public_id`) is gone. The test `v2_keeps_a_v0_1_0_history_and_seeding_claims_it` was removed with the legacy migration it tested; `v2_on_a_fresh_database` became `the_schema_creates_cleanly`. Added: `messages_are_indexed_by_triggers`, `memory_is_append_only`, `an_unexpected_user_version_is_refused`.
- The schema change forced `chat.rs`, `api.rs`, `auth.rs`, `admin_api.rs` and the tests to move to the new tables in this commit. `Db` methods now take a `user_id` and use ULID text ids. R2.9 builds on this (it adds the cross-user test), so R2.9 is smaller than listed.
- `messages_fts` is created and kept in sync by triggers here. A history-search query does not exist yet anywhere in the server (the list assumes one); see R2.12.
- Rust total after R2.8: 137 passed, 0 failed.

## R2.10: no code change

The proto `Conversation` and `Message` types already carried string ids, and `api.rs` and its tests moved to string ULIDs in R2.8 (the schema change forced it). Verified: no `public_id` or integer conversation/message ids remain in `askbar-server` or `askbar-proto`. Rust: 139 passed.

## R2.11: no code change

Done in R2.3 as agreed (see "Order change"): `Remote.ids` is gone, Tauri commands and stream events use string ids, and `ui/main.js:534` (now `row.dataset.id`), `ui.test.js` and `mock-tauri.js` use string ids. Verified with T1: clippy has no errors; Rust 139 passed; markdown 19, ui 39, admin 23 pass.

## R2.11a / Checkpoint R2b (T2)

- `scripts/smoke.sh` added. Against a throwaway server on an empty data directory: 10 PASS, 0 FAIL, 2 SKIP.
  - SKIP "search history": there is no search endpoint until R2.12. R2.12 must remove this fallback so a missing endpoint fails.
  - SKIP "does the desktop client show the streamed reply": **needs a person.** It asks y/n on a terminal; this session had none, so it was not checked and is not recorded as passed.
- The script's setup steps use today's admin API (password, master key if `key_set` is false, provider, pair code). It will need updating when the key step goes (R3) and again when the admin moves (R4).
- T1: clippy no errors; Rust 139 passed; markdown 19, ui 39, admin 23 pass.

## R2.12 notes

- Mismatch with the task list: it says to "move" history search onto `messages_fts`, but the server had no search at all. A minimal user-scoped `GET /api/v1/search?q=` (proto `SearchHit`, top 20, all words must match, query text is never FTS syntax, max 200 characters) was added on top of `messages_fts` so the smoke test and R2.12's own test have something to exercise. No client UI uses it yet. Revert this commit if search was meant to be out of scope.
- Tests: `search_finds_a_message_and_never_returns_another_users` (db) and `search_returns_only_my_matching_messages` (API). The smoke script no longer skips search when the endpoint is missing.
- Rust: 141 passed.

## R2.14 notes

- `providers.id` already existed as a ULID column (the list says to add it), so no schema change was needed.
- `ProviderInfo` gained `id`; `GET /api/v1/providers` returns `{id, name, type, model}` (the wire key for the kind stays `type`, as before). `ChatRequest.provider`, `store::load_provider` and the client's `active_provider` take the id; a name is rejected (404 on chat).
- Client: `ClientStatus.providers` is now a list of `{id, name}`; the "Use <name>" buttons call `set_active_provider` with the id. An old `client.yaml` that holds a name shows "provider no longer exists, pick another".
- `smoke.sh` picks the provider by id. Rust: 142 passed.

## R2.15 notes

- `MIN_CLIENT` is 0.3.0; a new `version_gate` test pins it and checks that a 0.2.x client is refused with 426. The other `version_gate` tests now use `MIN_CLIENT` and 0.3.0 versions.
- Side effect to know about: the desktop client's own version is still 0.1.0 until R5.4, so until then a client built from this branch is refused (426) by a default server. Tests that run a real client against a real server set the server's minimum explicitly (`with_min_client(.., "0.1.0")`); server tests that send a version header now send 0.3.0.
- Rust: 143 passed.

## R2.16 notes

- `GET /api/v1/me/display-defaults` returns the caller's row from `display_defaults`, falling back to the server-wide `display` setting, then the built-ins. Interim setter: `PUT /admin/api/users/{id}/display-defaults` (validated like the existing display settings; 404 for an unknown user). Tests in `tests/display_defaults.rs`. Rust: 145 passed.

## R2.17 notes

- `client.yaml` now holds only the server address and `allow_insecure` (pairing), the hotkey and the display overrides; `active_provider` is gone (an old key in the file is ignored). The device token stays in the keychain.
- The active provider is read from and written to `GET`/`PUT /api/v1/me/prefs`; with none chosen the client uses the first provider. Display defaults come from `GET /api/v1/me/display-defaults`.
- To make "resetting an override returns the default" true, `Remote::settings()` re-reads the hotkey and `ui` overrides from `client.yaml` each time the window opens (the server address is never re-read; it stays as paired). Before this, hand edits to `client.yaml` were only picked up at startup.
- There is no UI for editing overrides (they are still hand-edited in `client.yaml`). The `ui` Node tests drive the mock bridge: server default applies, an override wins for its own field, removing it returns the default. The behaviour itself is tested in Rust against a real server (`display_defaults_come_from_the_server_and_a_local_override_wins`).
- `tokio` in `src-tauri` now declares the `macros` feature explicitly. It only worked before through workspace feature unification, so a standalone `cargo build -p askbar` (what `npm run build` does) failed to compile `tokio::select!`.
- Tests: Rust 147 passed; ui 43 (was 39); markdown 19; admin 23.

## Fix after R2.15 / R2.17

`tests/admin.test.js` still sent client version 0.1.0, which `MIN_CLIENT` 0.3.0 (R2.15) refuses with 426, so 2 admin steps failed. R2.15 only ran T0, which does not include the Node tests, and it went unnoticed until R2.17's log step. Fixed by sending 0.3.0; admin steps are 23 again.

## R2.18 notes

`docs/wire-contract.md` written from the code (proto types and the route handlers). It states that chat answers `503` before the master key exists; R3.5 removes setup mode, so that line must be removed then.

## Fix: flaky tests that start the mock LLM

Two full test runs (R2.6 and the R2 gate) each reported one failure that did not reproduce. Running the suite with four busy loops reproduced it: `test_button_reports_success_and_failure_without_leaking_the_key` could not reach the mock LLM. Cause: `tests/mock_llm.py` printed its "ready" line before it bound the port, and the Rust tests wait for that line and then connect. The mock now binds first. Six runs under the same load pass.

## R2 gate: first build.yml run (sha ff2fd6e) failed on macOS and Windows

Linux passed everything (Rust tests, markdown tests, app build). The other two failed at "Rust tests":

- macOS: `mock did not start: Timeout`. My earlier "bind before announcing" fix made `mock_llm.py` print only after `HTTPServer.server_bind`, which calls `socket.getfqdn` and can take many seconds on a macOS runner. The mock now skips that lookup.
- Windows: `client_disconnect_saves_partial_reply_and_closes_upstream` never saw the disconnect, because the mock only caught `BrokenPipeError`/`ConnectionResetError` and Windows raises `ConnectionAbortedError` (WinError 10053). The mock now catches `ConnectionError`.

Both are test-infrastructure bugs in `tests/mock_llm.py`. Local run after the fix: Rust 147 passed, smoke 11 passed. The gate needs a new build.yml run.

## Checkpoint R2 gate

- T2 locally (sha ff2fd6e plus the mock fix in 2323caa): clippy no errors; Rust 147 passed; markdown 19, ui 43, admin 23; `scripts/smoke.sh` 11 passed, 1 skipped (the desktop-client check needs a person and is **not** verified).
- Manual `build.yml` run 41000972542 on sha 2323caa: **success on ubuntu-22.04, windows-latest and macos-latest** (Rust tests, markdown tests, app build). The earlier run 40999611639 failed on macOS and Windows for the mock-LLM reasons logged above. Windows and macOS are build-verified only, not run by hand.

## R3.1 notes

`secrets::ensure_key_file` is called at startup (replacing the `load_key_file` + "setup mode" warning). It loads an existing key, creates one (0600) on a first start, never overwrites, and refuses with an error naming the `askbar-secrets` volume if the key is missing while `providers.api_key_enc` holds data. The wizard/setup-mode code still exists until R3.2–R3.5, but a started server now always has a key. Two tests added; Rust 149 passed.

## R3.2 notes

- Admin UI: the wizard's master-key step and "save your key" screen are gone; setup is now just the password. The master-key panel (rotate, previous key) is gone.
- **Deviation:** the task says to remove "the Security tab". That tab also holds the change-admin-password form, which is not part of the deleted master-key feature, so the tab stays with only that form. Say if you want password change removed too.
- `admin.test.js`: the four steps for the key wizard, "key shown once" and rotation were replaced by two (setup then app opens with no key step; the server's own key exists at 0600 and the page never shows it). Admin steps 23 -> 21; the password-change step is kept. Rust unchanged (149).
- `/admin/api/setup/master-key`, `/admin/api/security*` still exist server-side until R3.3/R3.4.

## R3.3 notes

Deleted `rotate.rs`, the rotate/previous-key/security-status endpoints and their routes, `rotation.rs` (8 tests), and what only they used (`admin::reauth`, `secrets::new_path`/`prev_path`/`random_key`, the startup `rotate::recover`). Rust 149 -> 141 (the 8 rotation tests); admin 21 and smoke 11 still pass.

## R3.4 notes

Deleted the `generate-master-key` command, `secrets::generate_key` and `write_key_file` (key creation is now the one `generate_key_file`, which still never overwrites and writes 0600), `setup_master_key` and its route. In `admin_setup.rs`, `an_existing_key_can_be_restored_instead` was removed and `first_run_password_then_master_key` became `first_run_password` (its key assertions removed, password assertions kept). Rust 141 -> 140. README text about the wizard/rotation is stale until R3.7/R5.4.

## R3.5 notes

Setup mode is gone: `require_ready`, `setup_complete`, `set_secrets`, the `key_set` field of `/admin/api/setup-status` and every "setup is not finished" 503 are deleted, and `AppState` holds `Arc<Secrets>` (no `Option`, no `RwLock`, no `key_path`). `build` is now `(db, secrets, min_client, public_url)`. `setup_mode.rs` (1 test) removed. In `admin_network.rs` the second half of `network_needs_a_session_and_a_master_key` (the no-key 503) was removed with the feature and the test renamed `network_needs_a_session`. Login before any admin password is set still answers 503; that is "no password yet", not setup mode, and is kept. `docs/wire-contract.md` updated. Rust: 140 -> 139.

## Checkpoint R3a (T1)

- Rust: 139 passed, 0 failed (R2 gate 147; -8 rotation, -1 setup mode, -1 key-restore test, +2 key-file tests).
- Clippy: no errors, 5 pre-existing style warnings.
- Node: markdown 19, ui 43, admin 21 (-2 with the key wizard and rotation steps), all passing.
- A fresh start creates the key and the admin setup works without a key step: covered by `ensure_key_file` tests, the admin Node test (setup opens the app with no key screen; `master.key` exists at 0600), and `scripts/smoke.sh` (11 passed, 1 skipped for the person-only check). `smoke.sh` no longer has its master-key branch.

## R3.6 notes

`AppState` has one constructor, `AppState::new(db, secrets, min_client, public_url)`; `with_min_client`, `with_config` and `build` are gone. All 21 call sites (tests, `main.rs`, the client's `remote.rs` tests) updated; tests that used the old default pass `askbar_server::MIN_CLIENT` and `"http://localhost:8080"`. Rust 139 passed.

## R3.7 notes

README (first-run steps, "set on the admin page" list, Backup and recovery, the verified list) and the `compose.yaml` comment now say: the key is created automatically; back up `askbar-secrets` separately from `data/`; a raw `data/` restore with encrypted keys onto a new secrets volume makes the server refuse to start; no rotation. The full README rewrite is still R5.4.

## Checkpoint R3 gate (T2)

- T2: clippy no errors; Rust 139 passed; markdown 19, ui 43, admin 21; `scripts/smoke.sh` 11 passed, 1 skipped (person-only check, not verified).
- Manual checks against the real `askbar-server` binary with empty data and secrets directories:
  - First start created `master.key` (mode 0600); admin setup needed only a password.
  - A provider key stored before a restart still works after it (the admin Test button against the mock LLM returned ok both times).
  - The admin Backup file has `api_key_enc` unset on every provider, no `pair_codes`, and does not contain the stored secret.
  - With `master.key` deleted while a provider key was stored, the server exited with an error naming the `askbar-secrets` volume and did **not** create a new key.

## R4.1 notes

- `mgmt.rs`: a management router with its own listener on `ASKBAR_MGMT_ADDR` (rejects `0.0.0.0` and `::`; if unset, the listener is off and a warning is logged, until R4.13 makes it required). The bearer token is hashed (SHA-256) and compared in constant time (`subtle`) with the hex hash in the file named by `ASKBAR_MGMT_TOKEN_HASH_FILE` (default `/mgmt/token.sha256`), read on every request. A missing, empty or malformed file refuses everything. Every path (including unknown ones) needs the token, so there are no unauthenticated 404s.
- Not specified in the task: refusals are all `401` (no distinction between "no token" and "no hash file"), to avoid telling a caller which one it is.
- Tests (`tests/mgmt.rs`, 4, and one in `listeners.rs`): 401 with no token, a wrong token, or the stored hash used as the token; refused when the file is missing or junk and works again when it appears; a replaced hash revokes the old token at once; 200 on `/mgmt/v1/health` with the right token; the three routers share no routes; the wildcard address is refused. Checked once by hand with the real binary (401 without, 200 with the token). Rust: 144 passed.

## R4.2 notes

- Migration 2 (additive): `config_snapshot` (one row, `id = 1`, with `version`) and `redeemed_codes`. The `PRAGMA user_version` is now 2; the version-check tests use `MIGRATIONS.len()`.
- `askbar-proto`: `Snapshot` (providers' metadata only, default provider and fallback order, system prompt, server-wide and per-user display defaults, pair-code hashes, revoked device ids, `Budget` (daily/monthly token limits), `kill_switch`, `audit_retention_days`), `SnapshotProvider`, `SnapshotPairCode`. The task did not fix the exact field set; budget and kill switch are stored but nothing enforces them yet.
- `PUT /mgmt/v1/snapshot` is the only writer (`snapshot::save` is private to the crate); `snapshot::current` is the read-only accessor for the chat code. The server assigns `version` (1, 2, ...) and ignores any in the body. Rejected with 400 and nothing stored: retention below 7 days, duplicate/blank provider ids, an unknown provider type, a default/fallback id that is not a listed provider, display values out of range.
- Rust: 147 passed.

## R4.3 notes

- `PUT /mgmt/v1/providers/{id}/key` (write-only, `204` with no body, key encrypted with the master key; 400 for an empty or over-4096-character key, 404 for an unknown provider) and `POST /mgmt/v1/providers/{id}/test`.
- "Moving the logic out of `admin_api.rs`": the Test logic now lives in `provider_test.rs` and both the admin site and the management API call it (the old admin keeps working until R4.13). Key storage is new code in `mgmt.rs`, not a move: the old admin stores keys inside create/update provider, which stays as it is.
- A provider that is in the current snapshot but has no `providers` row gets one when its key is first stored (copied from the snapshot's metadata), so the new admin can create providers without the old admin. An unknown id is 404, a name clash is 409.
- Tests: key stored encrypted and decryptable, replaced, never in any response or error, no read endpoint, token required; the test endpoint against a fake OpenAI endpoint (ok, and a rejected key that the provider echoes back is shown as `[key hidden]`). Rust: 149 passed; admin Node tests still pass.

## R4.4 notes

- `GET /mgmt/v1/devices`, `GET /mgmt/v1/usage`, `GET /mgmt/v1/backup`, all bearer-authenticated.
- Moved out of `admin_api.rs`: the backup (to `backup.rs`: `VACUUM INTO`, scrub, temp-file cleanup, filename stamp and its test) and the device list (`auth::list_devices`); the admin site now calls the same code.
- The backup now also scrubs what R4.2 added: it empties `pair_codes` inside the stored snapshot document and deletes `redeemed_codes` (otherwise pairing-code hashes would leak through the snapshot). Existing admin backup tests still pass.
- "Usage" was not defined in the task and the admin had no usage logic; the endpoint returns counts (users, conversations, messages, active/revoked devices) and token totals summed from the reserved `token_usage` table (0 until something fills it).
- Rust 151 passed.

## Checkpoint R4a (T1)

Rust 151 passed, 0 failed; clippy no errors (same 4 pre-existing style warnings); Node: markdown 19, ui 43, admin 21.

## R4.5 notes

- `bridge.rs` (temporary, deleted in R4.13) rebuilds the snapshot from the old tables (providers, settings, `display_defaults`, unexpired unused pair codes, revoked devices) at start-up and after every successful non-GET `/admin/api/*` request. What the old tables cannot hold (default provider, fallback order, retention, budget, kill switch) is kept from the previous snapshot.
- Chat and the device API now read only the snapshot: provider settings (the API key is still read from `providers.api_key_enc`), the system prompt, and display defaults (`/settings`, `/me/display-defaults`). `GET /api/v1/providers` lists the snapshot's providers in client order: default provider, then fallbacks, then the rest by name (this is how the "default provider" reaches clients without a wire change; a client with no choice uses the first).
- This changed semantics slightly: a provider that is in the snapshot but has no stored key is now found by the test endpoint (it runs and fails at the provider) instead of 404; the R4.3 test was updated to say so.
- Tests that seed tables directly (`chat`, `read_api`, the client's `remote.rs` tests) now call `bridge::rebuild` after seeding, as the old admin would. New `tests/bridge.rs` (3): changing a provider in the old admin changes `GET /api/v1/providers` (create, edit, delete, refused change leaves it alone); the snapshot holds no key and is rebuilt at start-up; the client order follows default and fallbacks.
- T1: clippy no errors; Rust 154 passed; markdown 19, ui 43, admin 21; smoke 11 passed, 1 skipped (person-only).

## R4.6 notes

- `redeem_pair_code` now finds the code's hash in the current snapshot (unexpired), then inserts it into `redeemed_codes` (primary key = at most once, even for racing requests) and creates the device named from the snapshot entry. The old `pair_codes.used` flag is no longer written; the bridge leaves already-redeemed hashes out of the snapshot it builds.
- `authenticate` refuses a device whose id is in the snapshot's `revoked_device_ids` as well as one with `devices.revoked_at` set (the old admin still sets `revoked_at`; the new admin will push ids; both work).
- Every test that created a pair code and redeemed it directly now calls `bridge::rebuild` in between, as the old admin does after a change.
- New `tests/snapshot_auth.rs` (3): a code pairs once and a reused code fails (also after the admin pushes it again); unknown/expired/no-snapshot codes fail and six racing redemptions give exactly one device; a device listed as revoked only in the snapshot gets 401, and works again when the snapshot no longer lists it.
- **Disk:** the build tree reached 27 GB and filled the disk once mid-task; I removed `target/debug/incremental` and now build with `CARGO_INCREMENTAL=0`.
- One run of `another_user_cannot_read_or_continue_my_conversation` (chat.rs) timed out waiting for the reply right after the disk-full rebuild; eight reruns and the full suite pass. Unreproduced.
- T1: clippy no errors; Rust 157 passed; markdown 19, ui 43, admin 21; smoke 11 passed, 1 skipped.

## Checkpoint R4b (T2)

With the old admin still serving the UI: clippy no errors; Rust 157 passed, 0 failed; markdown 19, ui 43, admin 21 (the admin Node test drives the old admin UI end to end: setup, provider add/test/edit/delete, devices pair and revoke, network, settings, backup); `scripts/smoke.sh` 11 passed, 1 skipped (the desktop-client check needs a person; not verified).

## R4.7 notes

- New crate `askbar-admin` (in the workspace): `admin.db` with its own migration (`admin_credentials`, `providers` (metadata only, no keys), `settings`, `display_defaults`, `pair_codes`, `budgets`, `network_settings` with the IP allowlist off) and the same version guard as the server's database (application id "ASKA", so the server's database is refused); in-memory sessions and failed-login limiting (5 failures in 15 minutes lock logins for 15 minutes; a success clears the count); a unix-socket listener (`ASKBAR_ADMIN_SOCKET`, default `/run/admin/admin.sock`, mode 0660, replaces a stale socket) with `GET /health`; `askbar-admin healthcheck`.
- The login and setup handlers that use the sessions and the limiter come in R4.9; for now only `/health` is routed.
- Tests (6): migrations and safe defaults; persistence and refusal of a foreign/newer database; the limiter (blocked at the 5th failure, a success resets, failures outside the window do not add up); sessions; the socket answers health in-process; and the real binary: its socket answers, `healthcheck` exits 0 while up and non-zero after, `admin.db` is created, and none of its sockets is a TCP/UDP listener (read from `/proc`, Linux only).
- Rust: 163 passed.

## R4.8 notes

- `askbar-admin::mgmt`: `ensure_token` creates `mgmt.token` once (`mgt_` + 32 random bytes, base64url, mode 0600, directory 0700, never overwritten; an empty file is an error) in the admin's own secrets directory (`ASKBAR_ADMIN_SECRETS_DIR`, default `/admin-secrets`) and writes only its SHA-256 to `token.sha256` in the directory the server reads (`ASKBAR_MGMT_DIR`, default `/mgmt`; mode 0644 so the server's user can read it; rewritten if missing or wrong). `build_snapshot` builds the server's snapshot from `admin.db`. `ServerLink` pushes it (`PUT /mgmt/v1/snapshot`), retries with a pause of 1 s growing to 30 s while the server is unreachable, re-checks the server every 10 s, re-sends after the server comes back, and exposes `server_online()`, `last_version()`, `last_error()` and `pending()`; `mark_dirty()` is what the handlers of R4.10 will call after a change.
- Migration 2 of `admin.db` (additive): `revoked_devices`, because the snapshot's revoked ids need a home in the admin's database and R4.7 had none.
- Settings keys the snapshot reads from `admin.db`: `system_prompt`, `display`, `default_provider_id`, `fallback_order` (JSON list), `audit_retention_days` (default 30).
- Tests (5, against a real in-process server via `askbar-server` as a dev-dependency): the token is 0600, the hash file holds only the hash, restart keeps the token, a tampered or missing hash file is repaired; the snapshot is built correctly from `admin.db`; a push applies and versions go 1, 2; an unreachable server reads as offline while a wrong token reads as a 401 refusal; pushing retries until the server is up on that address and then follows later changes. Rust: 168 passed; clippy no errors.

## R4.9 notes

- Login, logout, first-run setup, change-password, the session check, the CSRF header check and the security headers are now in `askbar-admin::web` (same paths as before: `/admin/login`, `/admin/logout`, `/admin/session`, `/admin/api/setup-status`, `/admin/api/setup/password`, `/admin/api/password`; plus `/health`). The password lives in `admin.db` (`admin_credentials`).
- The old copies in the server (`admin.rs` and its tests `admin_auth.rs`, `admin_setup.rs`) are **kept** until R4.13: the old admin site must keep working after every commit, so "move" is done as copy now and delete in R4.13. The tests were ported to `askbar-admin/tests/admin_auth.rs` (11) and `admin_setup.rs` (4), the same counts as the server's. Changes in the port: the "device tokens are rejected on admin routes" test became "bearer tokens never open the admin" (the admin has no device tokens), the "admin cookie is ignored on API routes" test stays with the server (an admin service has no API router), and a new test checks the locked-down response headers.
- Rust: 183 passed (the temporary duplication is +15; it goes away in R4.13); clippy no errors.

## R4.10 notes

- `askbar-admin::api`: providers (list; create/edit/delete; the key goes to the server with `PUT /mgmt/v1/providers/{id}/key`; **Test** proxies `POST .../test`), devices (listed from the server, merged with the ids this admin revoked; revoke puts the id in the snapshot), pairing (stores only the code's hash, builds `askbar-pair:` from `https://<hostname>` when HTTPS is on, else `ASKBAR_PUBLIC_URL`), settings and per-user display defaults, backup, and `GET /admin/api/status` for the "server offline" banner (`server_online`, `pending`, `error`). Same paths as the old admin API so the UI can move unchanged in R4.11. Network settings, the static UI and Caddy follow in R4.11.
- Rule for anything that changes configuration (`mutate`): write `admin.db` inside a transaction, push the snapshot, send any API keys, and commit only if all of that worked; otherwise roll back. With the server unreachable every such call, and the ones that need it (device list, Test, backup), answer `502` "The server is offline. Nothing was changed." Reading providers and settings (the admin's own data) still works. If a key send fails after the snapshot was accepted, the server is sent the rolled-back truth again.
- The admin database lock is now an async lock held while waiting for the server (`AdminState.db`, `ServerLink::run` and the tests were changed to match); `build_snapshot` takes a plain connection.
- `admin.db` migration 3 (additive): `providers.has_key`, because the admin never stores keys but the page shows whether one is set.
- Server additions: `GET /mgmt/v1/users` (needed to pick the user a pairing code belongs to) and, on every pushed snapshot, deletion of stored keys of providers it no longer lists (so deleting a provider in the admin removes its key). `DEFAULT_SYSTEM_PROMPT` moved to `askbar-proto` so the admin can show it without depending on the server.
- Backup: one `.tar` with `server.db` (the server's scrubbed backup) and `admin.db` (a copy without the password hash, the Cloudflare token and pairing-code hashes). Hand-written ustar writer, no new dependency.
- Tests (`askbar-admin/tests/api.rs`, 8, against a real in-process server): provider lifecycle with the key only on the server and never in `admin.db`, the list, the snapshot or any response; server down gives 502 and saves nothing; status; a pairing string really pairs a device once, then revoke makes the server refuse it; the HTTPS pairing address; settings and per-user defaults reach the server with validation; the backup holds both databases and no secrets; every route needs a session. Plus a server test for users and key cleanup.

## R4.11 notes

- Moved into `askbar-admin` (copies; the server's originals stay until R4.13): `network.rs` (hostname/CIDR/token validation and the network API), the static UI (`src/ui/`, served at `/admin/`, `/admin/app.js`, `/admin/style.css`, `/` and `/admin` redirect), and a rewritten `caddy.rs`.
- Caddy is reached through its admin **unix socket** (`ASKBAR_CADDY_SOCKET`) with `reqwest::ClientBuilder::unix_socket` at `http://caddy/load`. The Cloudflare token file still goes to the shared `caddy-secrets` directory (`ASKBAR_CADDY_SECRETS_DIR`, mode 0600, removed while HTTPS is off).
- Generated Caddyfile: global `admin unix//run/caddy/admin.sock|0660` with no `origins` line (must be repeated in every config or Caddy would lose its admin listener; **the mode suffix is unconfirmed and gets checked against the real Caddy image in R4.16**), `askbar.<host>`-style API site `<hostname>` -> `server:8080`, and `admin.<hostname>` -> the admin's unix socket (`/run/admin/admin.sock` as Caddy sees it). `remote_ip` appears only when the allowlist is on, as `@outside not remote_ip ...` plus `abort @outside` (drops the connection) on the admin host only. The old `auto_https disable_redirects` line is gone so port 80 can redirect.
- Network settings: new `allowlist_enabled` flag (default off) next to the address list; turning HTTPS on no longer requires an allowlist (the old server required one). Turning the allowlist on requires at least one address. Ports are no longer part of the settings (everything is on 443); pairing uses `https://<hostname>`.
- The Cloudflare token is encrypted in `admin.db` with its own key `admin.key` in the admin's secrets directory (new `secrets.rs`; unrelated to the server's master key).
- UI changes: a "Server offline" banner (polls `/admin/api/status` every 5 s and shows at once on a 502), an allowlist checkbox, text for the new backup format and for the admin host name. The old UI with master-key screens is not copied in that form.
- Tests ported: `admin_network.rs` (6) and `caddy_apply.rs` (7 + 1 new), now with a fake Caddy on a unix socket and a real in-process server (`tests/common`); new: the allowlist matcher on, off, and with several addresses; the UI assets and headers. Rust: 215 passed; clippy no errors. `admin.test.js` (Node) still targets the old admin and is moved in R4.12.

## R4.12 notes

`tests/admin.test.js` now starts three real processes (the server with its management listener, `askbar-admin` on a unix socket, and the mock LLM) and drives the new admin UI through a small TCP forwarder in front of the socket (the admin has no TCP port). The old in-server admin is still started by the server (on an unused port) because it is not removed until R4.13. Changes: setup/key steps already gone (R3.2), the pairing address is the fallback `ASKBAR_PUBLIC_URL`, the network step checks the new `admin.<host>` text and the allowlist checkbox (off by default, addresses kept while off), the backup step reads the new `.tar` with `server.db` and `admin.db` and checks neither has the provider key or the server's master key, and three steps were added: with the server stopped the admin still loads and signs in and shows the "Server offline" banner; a change made while the server is away is refused with an "offline" message and saves nothing; when the server is back the banner goes and changes work. A 502 is now an expected browser console message. Admin steps: 21 -> 24, all passing (three runs in a row). T1: clippy no errors; Rust 215 passed; markdown 19, ui 43.

## Checkpoint R4c (T2, through the new admin)

- `scripts/smoke.sh` now starts the admin service too (own database and secrets, unix socket, no TCP port) and does first-run setup, the mock provider and the pair code through it with `curl --unix-socket`: 11 passed, 0 failed, 1 skipped (the desktop-client check needs a person; not verified).
- `tests/admin.test.js` (24 steps, three consecutive runs green) covers, in a real browser against the new admin: setup with a password only, providers with Test (the key never shown again), pairing and revoke (the revoked token gets 401), settings, network, backup (tar with both databases, no secrets), password change, and **the admin loading, signing out and in again, and showing the "Server offline" banner with the server stopped**, then recovering when it returns.
- T1 part: clippy no errors; Rust 215 passed; markdown 19, ui 43, admin 24.

## R4.13 notes

- **Removed from the server:** `admin.rs`, `admin_api.rs`, `admin_ui/`, `network.rs`, `caddy.rs`, `AdminAuth`, the Caddy/TLS state and ports on `AppState`, the R4.5 bridge, `argon2`, `create_pair_code`, `store::system_prompt`, `encrypt_b64/decrypt_b64`. `AppState::new` is now `(db, secrets, min_client)`. The server serves the device API and the management API only; `ASKBAR_MGMT_ADDR` is now **required** (and never a wildcard), `ASKBAR_ADMIN_ADDR` is gone.
- **Migration 3:** drops `settings`, `pair_codes` and `display_defaults`; `providers` is rebuilt with only `id` and `api_key_enc` (existing keys kept). Everything else about providers, the system prompt, display defaults and pairing codes now comes only from the snapshot. `PUT /api/v1/me/prefs` checks a provider id against the snapshot. `PUT /mgmt/v1/providers/{id}/key` creates the `(id, key)` row for any provider the snapshot lists.
- **Server binary check:** none of "Askbar admin", `app.js`, `style.css`, `<!doctype html`, "Sign in", `x-askbar-admin`, "cloudflare", "Caddyfile" or `admin_password_hash` appears in `askbar-server` (checked with `strings`); they all appear in `askbar-admin`. The only "master key" text left in the server is its own startup error message.
- **Tests removed with the code** (their behaviour moved to `askbar-admin` in R4.9-R4.11 or is covered by `askbar-server/tests/mgmt.rs`): server `admin_auth` (11), `admin_setup` (4), `admin_network` (6), `caddy_apply` (6), `admin_backup` (2), `admin_devices` (3), `admin_providers` (5), `admin_settings` (3), `bridge` (3), unit tests in `caddy.rs` (6) and `network.rs` (3). The admin crate has `admin_auth` 11, `admin_setup` 4, `admin_network` 6, `caddy_apply` 8, `api` 8; the server's `mgmt.rs` has 14. Behaviours that had no home are re-covered: the system prompt reaches the provider on the next message and is never served (`chat.rs`), backups leave no temp file (`mgmt.rs`), migration 2 -> 3 keeps provider keys (`db.rs`). `the_admin_cookie_is_ignored_on_api_routes` is not kept: the server has no login at all now, and `listeners.rs` asserts the API answers 404 to every admin path.
- `askbar_server::testing` (doc-hidden) gives tests a way to put the server in a known state through the same `snapshot::apply` the management API uses; tests that used `create_pair_code` or seeded provider rows now use it.
- T1: clippy no errors; Rust 166 passed; markdown 19, ui 43, admin 24; smoke 11 passed, 1 skipped.

## R4.14 notes

- `Dockerfile` has a shared `build` stage and two targets: `admin` (runs as uid **10001**, no TCP port, health check = `askbar-admin healthcheck` over the unix socket, state in `/admin-data`, `/admin-secrets`, `/mgmt`, `/caddy-secrets`, sockets in `/run/admin` and `/caddy-run`) and `server` (last stage, so a plain `docker build .` still gives the server; uid **10002**, API on 8080, `ASKBAR_MGMT_ADDR` deliberately has no default, health check = `askbar-server healthcheck`). The admin uid is 10001 as R4.16 assumes; the server moved from 10001 to 10002 so the two share no ownership (R4.17 checks that the server cannot read the admin's files). Anyone with a `DATA_DIR` owned by 10001 must chown it (README in R5.4).
- Both targets were built here and each health check was run inside a running container: server healthy, admin healthy (socket `srw-rw---- askbar-admin`), ids as expected. As before, building in this container needs the egress proxy's CA, so the build used a scratch copy of the Dockerfile that only adds it.
- `build.yml` has a new `images` job (ubuntu): builds both targets, starts each and waits for its health check via `docker exec`. **Not run on GitHub yet.**
- `askbar-admin` is gated to unix (`#![cfg(unix)]` on the crate and its tests, a stub `main` elsewhere) so `cargo test --workspace` still works in the Windows job of `build.yml`. This needs the next `build.yml` run to confirm on Windows and macOS.
- T0: Rust 166 passed.

## R4.15 notes

- `compose.yaml` rewritten: services `server`, `admin`, `caddy` (plus the optional `mock-llm` and `cpa` profiles, both on `edge`), fixed container names (`askbar-server`, `askbar-admin`, `askbar-caddy`). Networks: `edge` (Caddy, server, optional services) and `mgmt` (`internal: true`, subnet 172.30.0.0/24; server 172.30.0.2, admin 172.30.0.3; the admin joins no other network). Volumes: `askbar-secrets` (server), `askbar-mgmt` (admin rw, server **read-only**), `admin-secrets`, `admin-data`, `admin-run`, `caddy-run`, `caddy-secrets`, plus Caddy's data/config. `docker compose config` publishes only Caddy's `443` and `80`. Caddy `depends_on` server and admin `service_healthy`; the admin depends on nothing.
- Both mgmt addresses are fixed because the server must be given one specific address (a wildcard is refused) and the admin must know it. The first attempt let Docker hand 172.30.0.2 to the admin while the server was starting, so the admin now has its own fixed address too.
- `.env.example` reduced to `ASKBAR_PUBLIC_URL` and `DATA_DIR`; the port, uid and gid variables are gone (README still lists them; R5.4 rewrites it). `caddy/bootstrap.Caddyfile` now puts Caddy's admin API on `unix//run/caddy/admin.sock|0660` instead of TCP 2019.
- Test: `up -d` reaches healthy for server and admin, Caddy running (no health check of its own). Through the admin socket from a throwaway container on `edge`: first-run password setup 204, provider created 201 (this goes admin -> mgmt network -> server and back), pair code created, server `/api/v1/health` 200.
- **Found, for R4.16:** Caddy creates its admin socket as `root:root 0660`, so the admin (uid 10001) cannot connect to it yet.
- **`scripts/smoke.sh --url` not run for T3:** nothing is reachable from the host until Caddy has a hostname (only 443/80 are published and the admin is on a socket), so the script has nothing to point at. The equivalent steps above were done from inside the compose networks. Needs a decision on first-run access (see the report at the end).
- Images were built with the scratch Dockerfiles that add the egress proxy CA (the Caddy build included), not with `docker compose build`. The disk filled again during this task; cleared Docker's build cache and `cargo clean -p askbar-admin -p askbar-server`.

## R4.16 notes

- `Dockerfile.caddy` was already the Cloudflare-DNS build (`xcaddy build --with github.com/caddy-dns/cloudflare`); only its comment changed. The bootstrap file (R4.15) puts Caddy's admin API on `unix//run/caddy/admin.sock|0660`; the compose file builds it as the `caddy` service.
- **Fix found while testing:** the images created the users with `useradd --system --uid N`, which gave them gid 999. Both now get an explicit group with the same number (admin 10001:10001, server 10002:10002). Caddy runs as `user: "0:10001"` so its admin socket is `root:10001 0660`: the admin (uid/gid 10001) connects, nothing else can. (The `|0660` mode suffix works as written.)
- Verified in the running stack: Caddy's admin API refuses TCP (127.0.0.1:2019 inside the container and askbar-caddy:2019 from another container on `edge`: connection refused); uid 10001 connects through the socket (`GET /config/`); a throwaway container on `edge` as uid 10001 with only `caddy-run` can too; the admin's Network > Apply returned "Caddy is serving the configuration", and Caddy's loaded config has `admin.askbar.test.invalid` -> `unix//run/admin/admin.sock` behind a `not remote_ip 10.0.0.0/8` abort, and `askbar.test.invalid` -> `server:8080`.
- Certificates were not requested (the test host is not real).
- `smoke.sh --url` still not run (see R4.15).

## R4.17 notes

- `scripts/check-isolation.sh` runs against a running Compose stack (container names are fixed) and uses throwaway `python:3.12-slim` containers for network and socket checks. Sections: (1) with the server stopped the admin signs in, loads, and reports `server_online: false`; (2) the management port gives 401 with no token, with the token's *hash*, and with a wrong token; a second server container with no hash file refuses the hash too (401); Caddy and a container on `edge` cannot reach 172.30.0.2:8082, and nothing on the server's edge address answers on 8082; the admin is on one network; (3) the server mounts none of `admin-secrets`, `admin-data`, `admin-run`, `caddy-run`, `caddy-secrets`, cannot list `/admin-secrets`, has no admin socket, cannot write `/mgmt` (mounted read-only), runs as a different user from the admin, and the admin cannot read the server's `master.key`; (4) allowlist on: Caddy's loaded config aborts `not remote_ip <cidr>` on the admin site, off: the admin site has no address rule; Caddy's admin API refuses TCP and uid 10001 reaches it by socket.
- The allowlist check inspects Caddy's loaded configuration rather than sending requests from an outside address (no certificate can be issued for the test hostname, so the HTTPS port cannot be driven from here). The behaviour of the rules is covered by `askbar-admin/tests/caddy_apply.rs`.
- It changes the admin's network settings (a made-up hostname) and restarts the server container, so run it on a throwaway stack. Result: all checks pass.

## Checkpoint R4 (T3 gate)

- Compose stack up from the rewritten `compose.yaml` (images built with the scratch CA Dockerfiles): server and admin healthy, Caddy running, only 443 and 80 published; `scripts/check-isolation.sh` passes.
- T1: build ok, Rust 166 passed, clippy no errors (5 warning lines, none new in files touched here); markdown 19, ui, admin all pass; `scripts/smoke.sh` 11 passed, 1 skipped (desktop-client step needs a person).
- **Not done as written:** `scripts/smoke.sh --url <Caddy>`. There is no host-reachable URL before a hostname and certificate exist; the smoke steps through the server's real API were run locally (T2) and the equivalent first-run admin and pairing steps were run inside the compose networks (R4.15).

## R5.1: suite counts against the baseline (S4)

| Suite (baseline -> now) | Baseline | Now | Why it changed |
| --- | --- | --- | --- |
| client (`src-tauri` unit) | 14 (+1 ignored) | 19 (+1 ignored) | +5, tests added in the client during R2-R4 (not itemised in the notes) |
| `askbar-core` unit | 25 | gone | crate folded into `askbar-server` (R2.6); its tests are in the server's lib unit tests |
| `askbar-core` tests/streaming.rs | 8 | 8 (`askbar-server/tests/streaming.rs`) | moved with the crate |
| `askbar-proto` unit | 5 | 7 | +2, added with the snapshot types in R4.1 (not itemised) |
| `askbar-server` unit (lib) | 16 | 32 | includes the core unit tests; minus 1 `import` (R1), 6 `caddy.rs` and 3 `network.rs` (moved to the admin in R4.13); plus tests added in R2-R4 (key file, snapshot, migrations, mgmt); not itemised per test |
| `admin_auth` | 11 | 11 (`askbar-admin`) | moved in R4.9 |
| `admin_network` | 6 | 6 (`askbar-admin`) | moved in R4.11 |
| `admin_setup` | 5 | 4 (`askbar-admin`) | the setup-mode test went with setup mode (R3.3) |
| `caddy_apply` | 6 | 8 (`askbar-admin`) | +2 added in R4.11 (not itemised) |
| `admin_backup` 2, `admin_devices` 3, `admin_providers` 5, `admin_settings` 3 | 13 | 8 (`askbar-admin` tests/api.rs) | merged; the behaviours the old files covered are in `api.rs` (8) and `askbar-server/tests/mgmt.rs`; see the R4.13 notes for what had no home and was re-covered |
| `rotation` | 8 | 0 | feature deleted in R3.1 (named in the task) |
| `setup_mode` | 1 | 0 | feature deleted in R3.3 (named in the task) |
| `auth` | 7 | 7 | |
| `chat` | 4 | 6 | +2 added in R2-R4 (the notes mention the system-prompt-from-snapshot test) |
| `listeners` | 1 | 1 | |
| `read_api` | 4 | 6 | +2 added in R2 (search, prefs) |
| `version_gate` | 4 | 5 | +1 added in R2.15 (minimum client) |
| new: `display_defaults` 2, `snapshot_auth` 3, `mgmt` 13, admin `mgmt_link` 5, admin `service` 6, admin lib unit 9 | 0 | 38 | new for the split |
| **Rust total** | **138** (+1 ignored) | **166** (+1 ignored) | |
| Node `markdown` | 19 | 19 | |
| Node `ui` | 39 | 43 | +4 added in R2 (the notes record 39 -> 43) |
| Node `admin` | 23 | 24 | key wizard and rotation steps removed (-2, R3), three steps added for the offline banner and recovery (R4.12) |

No test was deleted other than those named above as removed with a deleted feature or moved with its code. The per-file test counts above are from `cargo test --workspace` at this commit. (Earlier notes say `mgmt.rs` has 14 tests; the run shows 13: that was a miscount.) Server+admin+proto unit totals were not reconciled test by test against the baseline names, only by count and by the removals recorded in the earlier notes.

## R5.2 notes

- `scripts/check-isolation.sh` gained section 5: the server binary contains none of "Askbar admin", "app.js", "style.css", "x-askbar-admin", "Caddyfile", "cloudflare", "admin_password_hash" (`grep -a` inside the image); the server image has no `askbar-admin` binary; the admin container mounts no `askbar-secrets` and has no `/secrets`; `admin.key`, `mgmt.token` (admin) and `master.key` (server), and `cf_token` when present, are mode 600; a backup fetched through the admin holds `admin.db` and `server.db` and none of a fresh provider key (a random marker), the plaintext pair code or `$argon2` (the password hash). All pass on the running stack.
- `token.sha256` is deliberately 0644: it is a hash, and the server (a different uid) must read it. The raw token (`mgmt.token`) is 0600 in the admin's own secrets volume.

## R5.3 (not done: needs a person)

A fresh install from empty volumes was exercised piecemeal by me (R4.15-R4.17: `docker compose down -v`, `up -d` from empty volumes, healthy, first-run password, provider, pair code, Caddy configured through the admin, isolation script). The manual run the task asks for, with the real browser, a real domain and a paired laptop client, has **not** been done and is left for a person. The first-run access question (below) must be answered first.

## R5.4 notes

- README rewritten for the split: no standalone mode (it is gone from the code), the three programs and the two networks, per-container table, how the management token and keys are kept, HTTPS/allowlist as the admin UI words them, upgrade notes (server uid 10001 -> 10002, removed `.env` variables and `tls` profile), three-part backup, layout, tests, and an honest "verified / not verified" section.
- **First-run access is written up as an open item, not solved:** the admin has no TCP port and Caddy serves `admin.<hostname>` only after the admin has been given a hostname. Neither the plan nor the task list says how a first-time visitor gets in; I did not invent one.
- Versions bumped to 0.3.0 in the four Cargo manifests, `package.json`, `tauri.conf.json` and `Cargo.lock`. The client sends 0.3.0 and the server's `MIN_CLIENT` is 0.3.0.
- T1: build ok; Rust 166 passed; clippy no errors; markdown 19, ui 43, admin 24; smoke 11 passed, 1 skipped.

## R5.5 notes (build.yml, run 6 on 6c46ef5)

- Windows: all steps green (Rust tests, markdown, app build). `images` job: green (both images build, both health checks pass). So the unix-only gating of `askbar-admin` works on Windows.
- Ubuntu and macOS **failed** in `askbar-admin` tests, both problems in the tests, not the product:
  - macOS: `path must be shorter than SUN_LEN`: the test directories (`$TMPDIR/askbar-admintest-<ulid>/caddy-run/admin.sock`) overflow the ~104-byte unix socket path limit on macOS temp dirs. Test directories and socket names are now short (`ab-<8 chars>`, `c/a.sock`); verified locally with a macOS-length `TMPDIR`.
  - Ubuntu: `the_binary_serves_a_socket_and_opens_no_tcp_port` never saw its socket, because the binary's secrets and management directories default to the container paths `/admin-secrets` and `/mgmt`, which a non-root runner cannot create (it passed here only because this sandbox runs as root). The test now sets `ASKBAR_ADMIN_SECRETS_DIR` and `ASKBAR_MGMT_DIR`.
- Rust: 166 passed locally after the fixes. A new `build.yml` run is needed to confirm; the laptop client re-test (R5.5) needs a person.
