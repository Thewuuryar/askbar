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
