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
