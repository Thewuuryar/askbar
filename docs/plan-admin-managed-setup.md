# Plan: set up and run the whole server from the admin site

Status: proposal. Nothing here is built yet.

## Goals

1. **Service control.** The admin site can start, stop and restart each service (`server`, `caddy`, `cpa`) securely.
2. **CPA wired automatically.** The CLIProxyAPI (CPA) API key is created and used by the admin; no `cpa-init show`, no copy and paste.
3. **OAuth without a terminal.** Connect a Claude (Anthropic) and a ChatGPT (OpenAI/Codex) login from the admin site.
4. **Model dropdowns.** Once a provider is reachable (CPA with a login, or any provider with URL and key), the model field is a dropdown of the models that provider lists.

## What exists today (relevant facts)

- `admin` is on `server-api` (internal, no route out) only. It reaches the server through the server API and Caddy through a unix socket. It has no Docker access and no network path to CPA.
- `cpa-init` (a one-shot container) writes `config.yaml` and `askbar-api-key` into the `cpa-config` volume. The admin never sees them. Logins need `docker compose run ... cpa-login` with published callback ports.
- Provider `model` is a free-text input (`askbar-admin/src/ui/app.js`, `#p-model`). Nothing lists models. Provider keys are held only by the server; the admin pushes them over the server API and the admin has no egress.
- `provider_test.rs` on the server already makes outbound provider calls on the admin's behalf. Model listing fits the same pattern.

## Design decisions

### D1. Service control goes through a new `controller` container, not the Docker socket in the admin

Mounting `/var/run/docker.sock` into the admin would make an admin compromise equal to host root, and would undo the admin/server isolation the architecture is built on. Instead:

- New service `controller` (small Rust binary in the workspace, `askbar-controller`). It is the only container with the Docker socket (mounted read-only is not enough on its own, so it is also restricted in code, see below).
- It exposes a tiny API on a new internal network `control` shared with the admin only, authenticated by a token whose hash it holds (same pattern as the server token: admin holds the token, controller holds the hash on a read-only volume).
- API: `GET /v1/services` (state of each allowed service), `POST /v1/services/{name}/{start|stop|restart}`. Nothing else: no exec, no create, no image pull, no arbitrary container names, no volume or network access.
- The allowed list is a constant in code: `server`, `caddy`, `cpa`. Targets are resolved by Compose labels (`com.docker.compose.project` equal to its own project, `com.docker.compose.service` in the list), so a request cannot name another container.
- Never controllable from the UI: `admin` (it would kill the request and cannot come back on its own) and `controller`. The UI offers "restart admin" only as a documented `docker compose restart admin`.
- Guard rails: `caddy` stop/restart shows a warning that the current page may disconnect (admin is served through Caddy); on first-run HTTP port 8081 it also drops. `server` stop warns that clients get "server offline" and running replies end as interrupted (existing behavior).
- Admin side: session-only routes (a script's API key cannot stop services by default; revisit if people ask), a CSRF header as for every change, a `service_control` audit line (who, what, when) written to the admin db, and a rate limit (a few actions per minute).
- Alternative considered: `tecnativa/docker-socket-proxy`. Rejected as primary because its allow rules are per Docker API verb, not per container name, so it cannot limit which containers; the bespoke controller can. It is a fallback if we do not want to maintain the controller.

### D2. The admin owns CPA's config and API key

- New volume ownership: `cpa-config` is mounted **read-write by the admin** and **read-only by cpa**. The admin generates `config.yaml` (API key, control secret) on first start, in the same format `cpa/init.sh` writes today, and stores the key and secret sealed in `admin.db` (using `secrets.rs`, same as the Cloudflare token).
- `cpa-init` and `cpa/init.sh` go away. Existing installs: on first start the admin finds an existing `config.yaml` and adopts its API key and control secret instead of replacing them (so existing logins and the current provider keep working). Covered by a migration test.
- New internal network `cpa-admin` (`internal: true`) with the admin and cpa. The admin gets no route out. The server still reaches cpa over `edge` as today.
- The admin creates the "CPA" provider itself (type `openai`, base URL `http://cpa:8317/v1`, key = the CPA API key, billing `subscription`) the first time CPA is up, and keeps its key in sync if the key is ever rotated. The Providers page shows it as "Managed by the admin"; name, URL and key are not editable for it. The "Add CLIProxyAPI" preset button is removed.
- CPA config changes (key rotation) are applied by the admin writing the file and asking the controller to restart `cpa` (or relying on CPA's file watcher; verify).

### D3. OAuth from the browser, using CPA's control API

CPA exposes a control API (login URL, callback, status, list/remove auth files). The admin proxies it so the browser never sees the control secret.

Flow (works when the admin is opened from another machine, with no published callback ports):

1. **Connect Claude / Connect ChatGPT** button. Admin calls CPA for an auth URL (`anthropic-auth-url`, `codex-auth-url`) and returns `{url, state}`.
2. Browser opens the URL. The user approves. The provider redirects to `http://localhost:<port>/...?code=...&state=...`, which will not load on a remote host. The UI says so and shows a paste box: "Paste the address you were sent to".
3. Admin validates the pasted URL (scheme/host/port/state match what it issued, state is still pending, 10 minute expiry) and forwards `{provider, code, state}` to CPA's callback endpoint.
4. Admin polls CPA's auth status for that state and shows "Connected as <account>" (from the auth file list). A **Disconnect** button removes the auth file.
5. Where the admin is opened on the Docker host itself, the same flow works; the paste box is simply the fallback. No `cpa-login` service, no published ports 54545/1455/8085/51121.

Security notes: control secret stays in the admin; only one login in flight per provider; pasted URLs are never logged; the auth list shows account label only, never tokens.

Open item to verify before coding: exact CPA v8.0.51 endpoint paths, parameters and response shapes (I could not reach the docs from this session). Pin them in a short `askbar-admin/src/cpa.rs` module with tests against a recorded fixture and a mock in `tests/mock_cpa.py`.

### D4. Model dropdowns via the server

The admin has no egress and no provider keys, so listing happens on the server:

- New server API route `POST /api/server/v1/models/list` taking `{type, base_url, api_key?, provider_id?}`; returns `{models: [{id, label?}]}` or a problem. With `provider_id`, the saved key is used (edit form, so the key is never re-typed); otherwise the key in the request is used once and not stored or logged (add form).
- Implementation per type: OpenAI-compatible `GET {base}/models` (also what CPA serves, filtered to the models its logged-in accounts can use); Anthropic `GET /v1/models` with `x-api-key` and `anthropic-version`. Handle pagination (`has_more`/`after_id`), a 10 s timeout, a cap on the response size, and redact the key from errors (reuse `redact` in `provider_test.rs`).
- Admin route `POST /admin/api/v1/providers/models` (API key allowed) forwards it. Admin route `GET /admin/api/v1/providers/{id}/models` for a saved provider.
- UI: the model field becomes a `<select>`. It loads when the form has what the provider needs (CPA: connected at least one login; others: URL and key present for `openai`/`anthropic`; debounce on blur, not on every keystroke). States: loading, list, error with a Retry button, and **"Enter a model name manually"** as an escape hatch for providers whose `/models` is missing (a few OpenAI-compatible servers). A saved model that is no longer in the list stays selected and is marked "not in list".
- Cache the list per provider for a few minutes in the admin to keep the form responsive.
- The same list can feed the default-provider/fallback settings pages if they show models.

### D5. Service status page

New admin tab **Services** (or a section of Settings):

- A card per service: `server`, `caddy`, `cpa` with state (running, stopped, restarting, unhealthy), uptime, health, and Start / Stop / Restart buttons with a confirm step that names the consequence.
- CPA card also shows: API key status (managed), connected logins (Claude, ChatGPT) with Connect / Disconnect.
- Refreshes on tab open and every 5 s while visible; buttons disabled while an action is in flight. Failures show the controller's reason.
- The Status endpoint (`/admin/api/v1/status`) gains a `services` summary so the first-run page can say "CPA is not running".

## Step order

Each step is its own commit with tests and the OpenAPI/docs change that goes with it (see CONTRIBUTING).

1. **Model listing, server side** (D4). Smallest, independent, immediately useful. Server route, `provider_test`-style module, unit tests with the mock LLM (`tests/mock_llm.py` gains `/v1/models`), `server.v1.yaml`.
2. **Model dropdown, admin side.** Admin routes, `admin.v1.yaml`, `app.js`/`index.html`, Playwright test in `tests/admin.test.js` (list loads, error, manual entry, edit keeps current model).
3. **Controller** (D1). New crate, Dockerfile target, `compose.yaml` (service, `control` network, token volume, Compose labels), unit tests for the allow list and label resolution against a fake Docker API, `docs/architecture.md` (volumes/networks tables, new "why this boundary exists" bullet), `SECURITY.md`.
4. **Admin service control.** `controller_link.rs` (mirrors `server_link.rs`), routes, audit lines, rate limit, Services tab, tests with a fake controller.
5. **CPA ownership** (D2). Admin writes config, adopts existing config, new `cpa-admin` network, remove `cpa-init`, auto-create the managed provider, migration test.
6. **OAuth connect** (D3). `cpa.rs`, routes, UI flow, `tests/mock_cpa.py`, tests for state expiry, URL validation and replay.
7. **Polish and docs.** README quick start becomes: up, open the admin, set passwords, Services > CPA > Connect, pick a model. Replace `cpa/README.md` with a short note; remove `cpa-login` profile; update `scripts/check-isolation.sh` for the new boundaries (controller reachable only from admin; admin cannot reach edge; cpa cannot reach admin ports).

## Security review checklist

- Docker socket mounted in exactly one container (`controller`), which has no published ports and is on one internal network with the admin.
- Controller token compared against a stored hash; admin cannot be tricked into passing a service name outside the allow list because the controller re-checks it.
- A compromised admin can restart or stop the three services but cannot start new containers, read volumes, or exec; this is the main residual risk and is stated in `SECURITY.md`.
- CPA control secret and API key sealed at rest, never returned by any admin endpoint, never logged.
- Pasted OAuth URLs: strict parse, state match, expiry, single use.
- Model-listing requests cannot be used as an SSRF probe beyond what provider setup already allows (same URL rules as providers; the request goes from the server, which already contacts arbitrary provider URLs). Decide whether to refuse link-local and metadata addresses for both listing and chat in one shared check.
- All new routes: session-only vs key-allowed decided per route, CSRF header on changes, problem+json errors, OpenAPI entries (the openapi tests enforce this).

## Risks and open questions

- **CPA endpoint details** are unverified (see D3). Mitigation: step 6 starts with a spike against the pinned image.
- **Restarting Caddy from the UI** can cut the admin's own connection. Mitigated by the warning; the page reconnects and re-checks status.
- **Windows/macOS Docker Desktop** socket paths differ; the controller takes the socket path from an env var, default `/var/run/docker.sock`.
- **Upgrade path** for people who already ran `cpa-login`: handled by adoption in D2; tested.
- **Rootless Docker / Podman** are out of scope for the first version; say so in the README.

## Decisions for you

1. Controller container (recommended) vs docker-socket-proxy vs socket in the admin (not recommended).
2. Should an admin API key (scripts) be allowed to start/stop services, or session only (recommended)?
3. Should the admin be restartable from the UI via a self-restart trick (exit and let `restart: unless-stopped` bring it back)? Recommended: not in v1.
4. Keep a way to enter the CPA key by hand (for a CPA you run elsewhere)? Recommended: yes, as a separate "Add provider" entry, since the managed one covers the bundled CPA only.
