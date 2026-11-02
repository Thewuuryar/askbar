# Hand-off: 0.5.0, one host, three audiences, one vocabulary

Branch `claude/askbar-server-side-chats-26xdjw`, 55 commits after the 0.4 hand-off (`ddaf234`), one per step of the plan. Nothing was pushed by this work and no pull request was opened. This file is not committed.

## What changed

- **Paths** follow `/<audience>/<...>/v<N>/<resource>`: client API `/api/client/v1`, server API `/api/server/v1` (internal listener only), admin API `/admin/api/v1`. The old paths are gone; `/api/v1/*` answers 426 "update the app" (D6). One host name: Caddy serves `/admin`, the client API, and a 404 problem for everything else. `docs/architecture.md` is the reference.
- **One vocabulary**: client (was device), server (was mgmt/management), admin. Renamed in code, SQL (migrations server v6, admin v7), env vars, the compose network (`server-api`) and token volume (`server-token`), the UI and docs. `scripts/check-names.sh` enforces it (first thing in `npm test`). Data volumes kept their names.
- **One error shape**: RFC 9457 problem details with a stable `error` code everywhere (server, admin, Caddy's 404); the stream's error event has a `code`; the desktop app maps by code, not words.
- **Admin API key** (one active, shown once, hash only, revocable, session-only routes for keys/password/session), made and revoked in the Security tab; **admin API public** with `admin.v1.yaml`.
- **OpenAPI 3.1** for all three APIs, with tests that fail if a route and its file disagree in either direction, or the `Problem` codes drift.
- **First-run access** to the admin on `http://127.0.0.1:8081/admin/` (a `setup` network only Caddy joins).
- **CLIProxyAPI** starts with the stack. Desktop Cargo package is `askbar-client` (program still `askbar`). Version 0.5.0, `MIN_CLIENT` 0.5.0.
- **Docs**: architecture, wire contract rewritten, README quick start and upgrade note, CONTRIBUTING, SECURITY, decision records in the refactor log.

## Results (saved in `/tmp/claude-0/-home-user-askbar/daac6cf8-503b-57c7-928c-03b0bcb4e537/scratchpad/`)

| Run | Result |
| --- | --- |
| `cargo test --workspace` (`r05-final-1-cargo.txt`) | 415 passed, 0 failed, 1 ignored (the client e2e needing a keychain) |
| `npm test` (`r05-final-2-npm.txt`) | passed: names check, docs check, cargo tests, markdown, UI and admin browser tests, mock LLM |
| `scripts/smoke.sh` (`r05-final-3-smoke.txt`) | 16 passed, 0 failed, 1 skipped (needs a person) |
| `scripts/check-isolation.sh` (`r05-final-4-isolation.txt`) | **not run**: no Docker daemon here |

CI was not triggered.

## Not verified, needs a person with Docker

1. **`scripts/check-isolation.sh`** has never run on this work. It now also checks the first-run page, the Caddy route that was loaded, that only Caddy is on the `setup` network, and CLIProxyAPI's network and ports.
2. **The first-run page (step 5.1).** Whether Docker publishes `127.0.0.1:8081` to Caddy's address on the `setup` network (rather than its `edge` address) when a container has two networks is untested. If `http://127.0.0.1:8081/admin/` does not answer after `docker compose up -d --build`, the approach needs rethinking (it stays unreachable from `edge` either way). The plan said to stop and report; I continued because nothing else depends on it, and the rest was verified without Docker.
3. **The generated Caddyfile was never loaded into a real Caddy** here; only its text is tested. The isolation script's section 4 reads the loaded config back.
4. **The pinned CLIProxyAPI image** was not pulled, and its usage passthrough is still unchecked (see the 0.4 hand-off).

## Plan deviations (each is in a commit body)

- **2.4 and 2.5 are one commit** (identifiers and SQL had to change together to compile).
- **2.9 (enforce the names check) is in 6.5**, because URL paths held the old words until then. 6.5 also fixed the server API's `/devices` route, which the path rename had skipped.
- **7.2 carries 7.3, 7.5 and the version bump of 7.8**, so the server, app, tests and scripts moved together.
- **3.8 changed the admin page's `api()` helper** (reads `detail`) so the UI kept working; 3.10 only adds the browser check.
- **5.8 puts the API key in the Security tab**, beside the password, not Settings.
- **6.3 added nothing**: 4.5, 5.1 and 8.7 already assert that `/api/server/*` is a 404 through Caddy and unreachable from `edge`.
- **4.1** (test-first cookie/CSRF tests) was skipped: `admin_auth.rs` already tests the cookie attributes, the header and the session.
- **8.5 uses `serde_yaml_ng`** (already in `Cargo.lock` for the desktop app) instead of `serde_yaml`.
- **LICENSE already existed** (AGPL-3.0-or-later); no step.
- Body errors that axum used to answer `422` are now `400 bad_request` (one admin test changed).

## Things to know

- The keychain entry name `device-token:<profile>` is kept on purpose (renaming would drop every stored token); its lines carry `names-ok`.
- `X-Askbar-Client-Version` stays, for "too old within v1" only.
- A wrong admin API key counts toward the login lockout, so a misconfigured script can lock out sign-in for 15 minutes.
- Two tests are timing-sensitive: `askbar-server/tests/report.rs` failed once under parallel load and passed on rerun; the Node admin tests use the built binaries, so run `cargo build -p askbar-server -p askbar-admin` first (a stale build looked like a failing step more than once).
- `SECURITY.md` says to use GitHub's private vulnerability reporting; I could not check it is enabled on the repository, and did not invent an e-mail address.
- Old `mgmt.token` in `admin-secrets` and the `askbar-mgmt` volume are unused after the upgrade.

## Manual checks

- Upgrade a copy of a real 0.4.0 stack (databases and volumes): it starts, paired apps keep working with the 0.5.0 app, a 0.4.0 app says it is too old.
- Open the admin at `http://127.0.0.1:8081/admin/` before any host name exists; then with HTTPS at `https://<host>/admin`; `curl https://<host>/api/server/v1/health` and `/` give a 404 problem.
- Create an admin API key, use it with curl, check it cannot create keys, revoke it, check it stops.
- CLIProxyAPI starts with `docker compose up -d` and idles; `cpa-login` still works with `--profile cpa-login`.
- The 0.4 manual checks (close the app mid-reply, six messages in one conversation, the 30 s timeout, the kill switch, the Audit tab with real CLIProxyAPI).

## Commits

```
dbcd80a 1.1: write docs/architecture.md
28d4caf 1.2: docs link checker
94b0100 2.0: CLIProxyAPI starts with the stack
b603d26 2.1: names check (report mode)
8f39a57 2.2: server crate modules named client_api and server_api
4ea1b4e 2.3: admin crate modules named admin_api and server_link
19ff85e 2.4: device becomes client in the code, with the database migrations (also 2.5)
bb722fd 2.6: env vars, network and token volume use the word server
e89bf2d 2.7: user text says client, server and app
c2d7268 2.8: the desktop Cargo package is askbar-client
6ed4090 3.1: problem details type and error-code registry
2e29a75 3.2: server error type and problem fallbacks
7a2825e 3.3: client gate, token check and pairing answer with problems
9bf14bd 3.4: chat refusals are problems
7a9c07e 3.5: the client read API answers with problems
c5919f7 3.6: the stream's error event carries a stable code
854eb9d 3.7: the server API answers with problems
648f608 3.8: the admin site answers with problems
653f381 3.9: the admin API and the audit proxy answer with problems
1713cf2 3.10: the admin page shows the sentence and keeps the code
bbf182d 3.11: the desktop app maps failures by the server's code
41b5f87 4.1: paths.rs holds every URL prefix
ac31893 4.2: Caddyfile tests for one host with path routing (ignored)
4ae68e9 4.3: one host, path-based Caddy routing
512c48a 4.4: wording for one host name
515d981 4.5: isolation script checks the path routing Caddy loaded
d905f3c 5.1: first-run access to the admin on the host's loopback
61cf6f3 5.2: the admin API lives under /admin/api/v1
a012a8d 5.3: the admin API names resources
9bc7829 5.4: the admin registers its routes through a table
6783783 5.5: the admin API key store
f574245 5.6: an admin API key opens the API
d761873 5.7: endpoints to make, list and revoke the admin API key
a94b9c9 5.8: the admin site makes and revokes the API key
6251ed8 5.9: the admin backup leaves out the API key
dbde095 6.1: the server API is at /api/server/v1
63f623f 6.2: the server API listener serves nothing else
d6d0372 7.1: tests for the client API at /api/client/v1 (ignored)
6865973 7.2: the client API moves to /api/client/v1 (hard cut), version 0.5.0
63f881a 7.4: Caddy sends only the client API to the server
614308c 7.6: /api/v1 answers 426 "update the app"
1dc55f0 7.8: wire contract and upgrade note for the new paths
400734a 6.5: the server API lists /clients; 2.9: the names check is enforced
b7a2879 8.1: the server registers its routes through a table
37ca497 8.2, 8.3: OpenAPI files for the client API and the server API
1eeece5 8.4: OpenAPI file for the admin API
81254cc 8.5: tests that keep the OpenAPI files true
921b21c 8.6: wire-contract.md links the specs and drops what they say
864963d 8.7: smoke and isolation scripts check the new shapes
4fb9854 9.2: CONTRIBUTING.md
929ed0f 9.3: SECURITY.md
8704fff 9.1: README starts with a quick start
face4ab 9.4: decision records in the refactor log
c94a8df 9.5: the docs and names checks pass over every file
063257e 10.0: the 0.5 hand-off report may describe the old names
```
