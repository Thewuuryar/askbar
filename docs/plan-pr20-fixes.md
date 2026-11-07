# Plan: fix the pairing panic and the CPA provider's missing key (PR #20 follow-up)

Status: approved by the owner on 2026-11-07, not built yet. The fixes go on PR #20 (step 1.9, branch `claude/nifty-albattani-7bns2k`) while it is open. Delete this file in the PR's last commit once both fixes are in, or keep it if the owner asks to.

## Context

The owner tested PR #20 on the Mac and found two problems.

### 1. Pairing panics the desktop app

The error is "Cannot start a runtime from within a runtime".

- **Cause (confirmed in the code):** `pair` is an async Tauri command (`clients/desktop/src-tauri/src/main.rs`). It calls `apply_config`, which calls `tauri::async_runtime::block_on(...)`. Blocking inside a tokio worker thread panics.
- **Not new in 1.9:** the bug came in with pairing. 1.9 is the first time pairing is used on the Mac.
- **Effect:** the token and the server address are saved before the panic. But the command never returns to the UI, so the pairing panel stays open, and the hotkey and look are not applied.
- **The other caller is fine:** the tray's "Reload config" calls `apply_config` from the main thread, where `block_on` works.

### 2. CPA answers 401 "Missing API key"

The model list and chat both fail this way (owner confirmed).

- The server sends no `Authorization` header when the provider's key is empty, both for the model list (`services/server/src/models.rs`, `fetch`) and for chat. So the server holds no usable key for the managed CPA provider. The admin still shows the provider as having a key.
- The admin never sends that key again:
  - `ensure_managed_provider` (`services/admin/src/cpa.rs`) returns early once the provider exists;
  - the snapshot push loop (`ServerLink::run` in `services/admin/src/server_link.rs`) sends only the snapshot, never keys.
- So once the server's copy of the CPA key is gone, CPA stays broken for good, even though the admin keeps the CPA key in its own files. Ways it can go: the server's database was replaced or reset, or the key was never stored.
- The code does not show which event emptied it on the Mac.

## Changes

### 1. `fix(client): apply the config after pairing without blocking the runtime`

All in `clients/desktop/src-tauri/src/main.rs`.

1. Split `apply_config` in two:
   - `apply_settings(app, &Settings) -> Option<String>` does the sync work: it stores `hide_on_blur`, sets `placement.top_percent` and calls `register_hotkey`.
   - `apply_config(app)` keeps `block_on` around `settings()`, then calls `apply_settings`. Add a doc comment: call it only from the main thread (the tray menu), never from an async command.
2. In `pair`, replace `let _ = apply_config(&app);` with:
   - `let settings = state.backend().settings().await;`
   - then, `if let Ok(cfg) = settings`, call `apply_settings(&app, &cfg)`.
3. Add a test. A unit test can't build a Tauri `AppHandle`, so add a `#[cfg(test)] mod tests` at the end of `main.rs` that checks the source instead:
   - read the file with `include_str!("main.rs")` and split it on `#[tauri::command]`;
   - for each command that starts with `async fn`, take its body up to the first `"\n}\n"`;
   - assert the body contains neither `apply_config(` nor `block_on(`;
   - also assert that more than 5 async commands were checked, so the test can't pass by finding none.
   
   Run it with `cargo test --bin askbar` from `clients/desktop`, after checking the binary's name in `src-tauri/Cargo.toml`.

### 2. `fix(admin): send the CPA provider's key to the server again whenever the server needs it`

**`services/admin/src/cpa.rs`:**
- Add a helper `managed_key(state) -> Option<(String, String)>`. It returns the managed provider's id (`managed_id`) and the CPA API key (`CpaLink::api_key()`, from `state.cpa`), when both exist.
- In `ensure_managed_provider`, when the provider already exists, `put_key` the CPA key to the server again before returning the id.
  - The call is idempotent.
  - This code runs when the Services tab loads (with accounts connected) and after a login, so opening Services repairs a broken CPA provider.

**`services/admin/src/server_link.rs`, `ServerLink::run`:**
- After every successful snapshot push, also `put_key` each key that a resend callback returns. Pass the callback into `run`, for example `resend: impl Fn() -> BoxFuture<Vec<(String, String)>>`, or an `Arc<dyn Fn ...>`.
- Wire it where `run` is spawned (`services/admin/src/main.rs` or `lib.rs`) to `cpa::managed_key`.
- If a resent key fails, log it and set the link dirty again, the same way a failed push is handled.
- Result: when the server restarts, or comes back with an empty database, the admin pushes the snapshot (`needs_push`) and the CPA key follows, within one check (12 s or less).

**Test in `services/admin/tests/api.rs`, against a real in-process server, using the helpers there:**
- Make a managed provider. Reuse a CPA fixture from the `cpa.rs` tests if one exists; otherwise make a `CpaLink` with a fixed key, or insert the `managed = 1` row plus a key file the way `CpaLink` reads them.
- Clear the server's `providers.api_key_enc`, the way a reset would.
- Trigger a push. Check that the key is back, and that listing models through a mock which requires that bearer key succeeds.

### 3. Reproduce first

Do this in the sandbox (see "Sandbox notes"), before writing the fix.

1. Start the stack.
2. Get a managed CPA provider: CPA with no accounts still enforces its API key, so no OAuth is needed.
   - Read the CPA key from the admin's CPA key file.
   - Create the `managed = 1` row through the same path the test uses, or create an ordinary `openai` provider at `http://cpa:8317/v1` with that key.
3. Null the server's key.
4. Load the model list: CPA should answer "Missing API key".
5. After the fix, the same steps should give 200 and an empty list.

## Not in this fix (tell the owner)

- **Ordinary providers can lose their key the same way.** If the server's database is replaced, a key the owner typed for an ordinary provider is gone too. The admin can't resend it, because it doesn't keep keys, and its form still says "unchanged (write-only)". A possible follow-up: the server reports which listed providers have no key, and the admin shows "key missing on the server, enter it again". Offer it; don't build it.
- **Ask the owner** whether the Mac's server data (`./data`) was reset during testing. That would explain the missing key.

## Verification

- **Client:**
  - `cargo test` in `clients/desktop`, including the new source check;
  - the UI tests: `NODE_PATH=$(npm root -g) node tests/ui.test.js`.
- **Admin:** `cargo test -p askbar-admin`, including the new test.
- **Whole repo:**
  - `NODE_PATH=$(npm root -g) npm test` and `scripts/smoke.sh`;
  - `scripts/check-isolation.sh` is optional, since no compose, network or volume changes. Run it anyway if the stack is up for the reproduction.
- **Sandbox reproduction:** run the steps above, before and after the fix.
- **For the owner on the Mac:**
  - pair a new app: no panic, and the panel closes;
  - open Services, then Providers > CPA > load models: the list loads;
  - chat through CPA works.
- **PR #20 body:** update the Verified and Not verified sections, and add the two fixes under "What and why".

## Instructions for the agent that picks this up

Read `AGENTS.md`; it overrides your defaults. Then:

1. Check out `claude/nifty-albattani-7bns2k` and pull it. PR #20 must still be open.
   - If PR #20 was merged, restart the branch from `main`, keeping the same branch name, and open a new PR for these fixes.
   - Never push to `main`, merge, or force-push.
2. Do change 1, then change 2, as **two separate commits**:
   - Conventional Commits, as named above;
   - the body says why;
   - end each commit message with the owner's attribution lines.
3. Delete this plan file in a final `docs:` commit, unless the owner says to keep it.
4. Run the verification above. Report exactly what passed and what you could not run. Never imply a check passed.
5. Push with `git push -u origin claude/nifty-albattani-7bns2k`, then update the PR #20 body. Follow `.github/pull_request_template.md`, and keep the closing lines it already has.
6. Stop and ask before anything that touches a hard rule in `AGENTS.md`, a network, a volume, or a new dependency.

### Sandbox notes (cloud sessions)

- **Docker:** start `dockerd` by hand if `docker info` fails.
- **Building images:** build them through a scratch copy of the `Dockerfile` with two lines added after `FROM rust:1-bookworm AS build`:
  - `COPY --from=ca ca-bundle.crt /ca.crt`
  - `ENV HTTPS_PROXY=http://127.0.0.1:<port> https_proxy=... SSL_CERT_FILE=/ca.crt CARGO_HTTP_CAINFO=/ca.crt`, where the port is `${HTTPS_PROXY##*:}`.
  
  Then build with `docker build --network host --build-context ca=/root/.ccr -f <scratch file> --target server -t askbar-server .`, and the same with `--target admin -t askbar-admin`.
- **Running the stack:** `DATA_DIR=<an empty scratch dir owned by 10002:10002> docker compose up -d --no-build`. A root-owned `./data` stops the server from opening its database.
- **Tests:** Playwright tests need `NODE_PATH=$(npm root -g)`.
- **Disk:** keep `target/` directories trimmed. The disk allowance is tight.
- **Formatting:** `cargo fmt --check` fails on `main` too (rustfmt is still proposed). Don't reformat unrelated code.
