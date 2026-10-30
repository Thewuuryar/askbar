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
