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
