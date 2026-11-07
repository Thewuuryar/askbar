# Askbar

A Spotlight-style AI popup. Press a hotkey, a bar appears in the upper-middle of the screen, type a question, read the answer, press Esc.

Askbar is an open-source project for personal hardware, with no support or uptime guarantee.

This page describes the code on `main`. The accepted v1 target differs: see [docs/architecture.md](docs/architecture.md] and the [ADRs](docs/adr/README.md). Where they disagree, build to the ADR.

- **Conversation only.** The model gets text in and returns text out. There are no tools, no shell, no file access, and the window is granted no filesystem or network permissions (`clients/desktop/src-tauri/capabilities/default.json`). Only the Rust side talks to your provider.
- **Any provider.** Anthropic, or anything that speaks the OpenAI `/chat/completions` protocol (OpenAI, OpenRouter, Ollama, LM Studio, vLLM, Groq, ...).
- **History** you can toggle (Ctrl+H), reopen and delete. Stored on your server, shared by every app you pair.
- **Copy in and out.** Paste anything into the bar. Copy a whole reply, a single code block, or the last reply (Ctrl+Shift+C).
- **Replies run on the server.** Close the window or lose the connection and the reply keeps going; reopen the app and it is there.
- **Keys stay on the server.** The desktop app is a thin client; provider API keys never reach it.

## Quick start

You need Docker with Compose for the server, and [Rust](https://rustup.rs) and Node 18+ for the desktop app.

```sh
cp .env.example .env
mkdir -p data
docker compose up -d --build
```

To stop it, or before rebuilding after an update, use `docker compose --profile dev down --remove-orphans`. With `--profile dev`, the mock LLM is removed as well; without it, a dev container left behind can point at a network that no longer exists.

1. Open **https://localhost/admin** on the machine that runs Docker, or `https://<its address>/admin` from another machine on your network, and accept the certificate warning (the install uses its own certificate until you set a host name).
2. Sign in with the published login, **`admin` / `admin`**, and change the password.
3. Choose the **audit passphrase**: at least 20 characters, and it cannot be recovered. It seals what the server records about replies; see *The Audit tab*.
4. **Providers** > add one (Anthropic or OpenAI-compatible; the model is a dropdown once the address and key are in) and press **Test**, or connect a Claude or ChatGPT account on **Services** (see *CLIProxyAPI*).
5. **Clients** > add a user, then generate a pairing key for them.
6. Run the app and pair it: `cd clients/desktop && npm install && npm run dev`, then tray icon > **Pair with server...** and paste the key.

That is a working install on one machine. For other machines, put it behind a host name with HTTPS (*HTTPS with your own hostname* below). How the pieces fit, and why, is in [docs/architecture.md](docs/architecture.md); the HTTP APIs are described in [docs/openapi/](docs/openapi/) (client, server, admin); contributing is in [CONTRIBUTING.md](CONTRIBUTING.md) and security in [SECURITY.md](SECURITY.md).

Askbar is three programs:

- **Desktop client** (Tauri): the popup. It pairs with one server and holds only its address, a hotkey and a client token (in the OS keychain).
- **`askbar-server`**: the client API. It stores conversations, runs the chat against your providers and keeps provider keys encrypted. It has no web UI and no login.
- **`askbar-admin`**: the admin site. It owns all configuration (providers, system prompt, display defaults, pairing codes, clients, network and HTTPS settings, limits and the kill switch) and pushes it to the server over a private server API. Its **Audit** tab shows what the server recorded about every reply.

There is no standalone mode: the client always talks to a server.

```
 desktop app --HTTPS, bearer token--> Caddy :443 --> askbar-server :8080 --> your providers
 browser     --HTTPS, session cookie-> Caddy :443 --unix socket--> askbar-admin
                                                                      |  server API (token, own network)
                                                                      +--> askbar-server
```

## Run the desktop app

Prerequisites: [Rust](https://rustup.rs), Node 18+, and your platform's WebView.

| Platform | Needs |
|---|---|
| Linux | `build-essential libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev` |
| macOS | Xcode command line tools |
| Windows | Visual Studio Build Tools (C++), WebView2 (preinstalled on Windows 11) |

```sh
cd clients/desktop
npm install
npm run dev        # run it
npm run build      # installers land in clients/desktop/target/release/bundle/
```

The app is its own Cargo workspace (`clients/desktop/`), so building the server needs none of these libraries.

Until the app is paired it shows how to pair. Tray icon > **Pair with server...**, paste the pairing key from the admin site (Clients), or make one in an app you already paired (tray icon > **Pair another app...**). A key works once and expires after 10 minutes; the app exchanges it for a per-client token stored in the OS keychain, not in a file. Revoke a client in the admin site and the app shows "token rejected". If the server can't be reached the app says "server offline"; there is no offline mode.

The app reads the token from the keychain once per launch. On macOS a build from `npm run dev` is unsigned, so after each rebuild macOS asks once for the keychain password before that build can read the token; **Always Allow** covers that build until the next rebuild.

`client.yaml` (`~/.config/askbar/` on Linux, `~/Library/Application Support/askbar/` on macOS, `%APPDATA%\askbar\` on Windows; `ASKBAR_CLIENT_CONFIG` points at a different file, which is how you run a second profile) holds only:

```yaml
server_url: https://askbar.example.com
hotkey: "Ctrl+Shift+Space"
allow_insecure: false          # a client refuses http:// unless the server is on this machine
ui:                            # optional; anything left out follows the server's defaults
  theme: dark
  width: 640
```

The active provider and your other preferences are stored on the server per user. Tray icon > **Reload config** applies `hotkey` and `ui` changes.

## Using it

| Key | Action |
|---|---|
| your hotkey | show / hide |
| Enter / Shift+Enter | send / new line |
| Esc | stop a reply that is streaming; otherwise hide |
| Ctrl+H | history panel |
| Ctrl+N | new conversation |
| Ctrl+Shift+C | copy the last reply |

In the history panel, click a conversation to reopen it. Delete needs two clicks ("Delete" then "Delete?"), and removes the conversation and its messages. Ctrl is used on macOS too, since Cmd+H and Cmd+N belong to the system.

## Replies run on the server

A reply is produced by the server, not by the window. The question is saved first and the server writes the answer as its own task, so closing the app, sleeping the laptop or switching networks does not stop it. When the window comes back it asks for the rest of the reply and picks up where it left off, without repeating or dropping text; a second client paired to the same server can open the conversation and watch the same reply.

- **Waiting messages.** You can send another message while a reply is running. Up to **5** messages can wait in one conversation, and they run one after another; a sixth is refused ("Five messages are already waiting...") and your text is kept. A waiting message can be removed from the window.
- **Stop** (Esc or the Stop button) tells the server to stop the reply; the text so far is kept.
- **Time limit.** A reply that runs longer than **300 seconds** is stopped; the text that arrived stays, the window says "Stopped after 5 minutes" and offers **Try again**. The limit is the **Generation timeout** in the admin site's Settings (30 to 3600 seconds).
- **Restarts.** If the server is restarted mid-reply, the reply ends as interrupted and keeps the text it had.
- **Refusals.** When the administrator has switched replies off (the kill switch in Settings), a daily or monthly token limit is used up, or the audit key has not been made yet, the window says so in words and keeps what you typed. A busy server is tried once more by itself.
- **Failed questions are not resent.** A question that got no reply stays in the conversation but is not sent to the model with your next one, so a question the provider refuses can't stick the conversation. To retry it, send it again (or **Try again** after a timeout).
- If another provider than the one you chose answered (the server fell back), the reply says "Answered by ...".

## Hotkeys by platform

`Super+A` is accepted (`hotkey: "Super+A"`) and works on X11, but Windows reserves it for Quick Settings and GNOME for the app grid, so the default is `Ctrl+Shift+Space`. If a hotkey can't be registered, the window opens with the reason.

**Wayland** does not allow ordinary apps to register global hotkeys. Bind a shortcut in your desktop's keyboard settings to the command `askbar`. Running it while Askbar is already running toggles the window instead of starting a second copy. This also works on any platform.

## Server

Needs Docker with Compose.

```sh
cp .env.example .env
docker compose up -d --build
```

That starts three containers on two networks:

| Container | Runs as | Network | Published |
|---|---|---|---|
| `askbar-caddy` | root (group 10001) | `edge` | **443, 80** |
| `askbar-server` | uid 10002 | `edge`, `server-api` | nothing |
| `askbar-admin` | uid 10001 | `server-api` only | nothing (a unix socket) |

`server-api` is an `internal` network shared by the admin and the server's server API (172.30.0.2:8082, fixed in `compose.yaml`). Caddy is the only thing the outside world can reach.

Every container has a read-only root filesystem, no Linux capabilities (Caddy keeps only the one to bind 80 and 443), no privilege gain, and limits on processes and memory; it writes only to its own volumes and a private `/tmp` ([ADR 0032](docs/adr/0032-container-hardening.md)). Caddy, though root, reads the admin's certificate key and Cloudflare token (mode 640) and makes its admin socket through group 10001.

- The **server** serves the client API, which accepts only `Authorization: Bearer <client token>`. Chat replies stream as Server-Sent Events (`meta`, `provider`, `delta`, `done`, `error`) and can be resumed after a dropped connection. Client tokens are random, shown once, and stored as SHA-256 hashes. See [docs/wire-contract.md](docs/wire-contract.md).
- The **admin** listens on a unix socket only. Caddy proxies the site (`<hostname>/admin`) and its API (`<hostname>/api/admin/v1`) to it, and the admin owns `admin.db`. Every change is made in `admin.db` and pushed to the server as one snapshot; if the server is down the change is refused ("The server is offline. Nothing was changed.") rather than half applied. With the server stopped the admin still loads and shows an offline banner.
- The **server API** requires a bearer token. The admin keeps the token in its own secrets volume (mode 600); the server holds only its SHA-256 (`token.sha256`, read per request, compared in constant time, and the API refuses everything when the file is missing).
- **Provider API keys** are encrypted at rest (ChaCha20-Poly1305) with a master key that the server creates on its first start in the `askbar-secrets` volume. They are sent to the server by the admin, stored only in `server.db`, and never returned by any endpoint. The Cloudflare token is encrypted in `admin.db` with the admin's own key (`admin-secrets`), separate from the server's, and written to a file only Caddy reads.

### First run

From the first start Caddy serves HTTPS on port 443 of every address of the host, with a certificate the admin makes for this install (port 80 redirects). Open **`https://localhost/admin`** on the machine that runs Docker, or `https://<its address>/admin` from another machine, and accept the browser's warning. Until you set a host name, the admin site answers only private addresses (loopback, 10/8, 172.16/12, 192.168/16, Tailscale's 100.64/10 and the IPv6 equivalents) unless the allowlist names others. Caddy checks the connection's real source; a router or proxy that rewrites sources can make public traffic look private. A pairing key made now points at the address you opened the admin with and carries the certificate's fingerprint, so the app trusts exactly this certificate for this server (set `ASKBAR_PUBLIC_URL` in `.env` to choose another address). Once a host name and a real certificate are set (step 5) use `https://<hostname>/admin`.

1. Open the admin (above).
2. Sign in as **`admin`** with the password **`admin`**. Every page but the password change refuses (403 `password_change_required`) until you choose your own: at least 10 characters, not `admin`. Anyone on a private address can use the published login until you do, so do it as soon as the stack is up.
   - **Locked out?** On the host, `docker compose exec admin askbar-admin reset` puts the published login back (you must change it again at the next sign-in) and records the reset on the Services tab; then `docker compose restart admin` to sign out every session. If the admin is not running, use `docker compose run --rm admin reset` instead (`run` cannot start beside a running admin: they would share its fixed network address).
3. **Audit passphrase.** Setup asks for one: at least 20 characters. See *The Audit tab* below; write it down.
4. **Providers** > add one (Anthropic or OpenAI-compatible: after you enter the address and key, **Model** becomes a dropdown of what the provider offers) and press **Test**. For a Claude or ChatGPT subscription use **Services** instead: *Connect Claude* / *Connect ChatGPT* (see *CLIProxyAPI*); no terminal is needed.
5. **Network** > hostname, Cloudflare token and, if you want it, the allowlist (see below).
6. **Clients** > add a user, then generate a pairing key for them.

No secret ever goes in `.env`, so nothing sensitive can be committed by accident.

### HTTPS with your own hostname

Certificates come from Let's Encrypt using a DNS challenge on Cloudflare, so the hostname may point at a private address.

1. In Cloudflare, add an `A` record for your hostname (for a private server, its LAN address), and create an API token with **Zone:DNS:Edit** and **Zone:Zone:Read**.
2. Admin site > **Network**: tick HTTPS, enter the hostname and the Cloudflare token. Leave **staging** on for the first try, then turn it off for a trusted certificate.
3. The apps reach the server at `https://<hostname>` and the admin site is at `https://<hostname>/admin`. One name, one certificate; every other path is a 404.

The admin allowlist is **off by default**: anyone who can reach `https://<hostname>/admin` can try to log in (login is rate limited). Tick **Only let these addresses open the admin site over HTTPS** and list addresses or CIDRs (for example `192.168.1.0/24`) to make Caddy drop every other address before it reaches the admin. If you reach the admin from the same machine through Docker, the connection can appear to come from the Docker gateway (for example `172.x`), so include that range.

### The Audit tab

The server records a trace of every reply: when, for whom, through which provider, how long each step took, how many tokens it used, which providers were tried and how it ended. The admin site's **Audit** tab shows it as an overview, a list of requests, a timeline for each one, routing (which providers answered and how often they failed), and an access history.

The **content** of messages (prompts, replies, provider errors) is *sealed*: the server encrypts it with a public key that is made from your audit passphrase (Argon2id to an X25519 key) and never holds the passphrase or the private key. It cannot read what it recorded. To read content, open a request in the Audit tab and type the passphrase; the admin derives the key in memory, opens that one item, and the opening is written to the access history (the request, the key and the time, never the content).

- The passphrase is chosen during setup and **cannot be recovered**. If you lose it, the sealed content recorded with it can never be opened; the rest of the trace (times, tokens, routes, outcomes) stays readable.
- **Settings > Rotate the audit key** makes a new key from a new passphrase. Content recorded earlier stays sealed with the old key and still needs the *old* passphrase.
- Records older than **Keep audit records for (days)** (default 30, at least 7) are deleted once a day. Nothing younger than 7 days can be deleted.
- Until a key exists the server refuses to start replies (`503`), rather than record them unsealed.

The trace lives in `audit.db`, in the `audit-data` volume, which only the server mounts; the admin reads it through the server API.

### CLIProxyAPI

Lets a Claude or ChatGPT subscription act as an OpenAI-compatible provider. It starts with the stack and idles until you connect an account. Everything is done in the admin site, **Services** tab:

1. **Connect Claude** (or **Connect ChatGPT**). The admin asks CLIProxyAPI for a login address.
2. Open the address and approve. The browser then lands on an address starting `http://localhost` that will not load if the admin is on another machine. Copy that whole address from the address bar and paste it into the box. **Finish**.
3. A provider named **CPA** appears on the Providers tab, already holding the key the admin made for CLIProxyAPI. Choose its model from the dropdown. Its name, address and key are fixed.

The admin makes CLIProxyAPI's config and API key itself (nobody copies a key anywhere). CLIProxyAPI joins the `edge` network (the server reaches it there) and `cpa-admin` (the admin runs logins there); its API is never published. See [deploy/cpa/README.md](deploy/cpa/README.md).

### Configuration reference

**`.env`** (read by Compose; nothing secret):

| Variable | Default | Meaning |
|---|---|---|
| `ASKBAR_PUBLIC_URL` | empty | address put in pairing keys until HTTPS is configured; with HTTPS on it is `https://<hostname>` |
| `DATA_DIR` | `./data` | where the server's database lives on the host; must be writable by **uid 10002** |

**Upgrading from 0.4 to 0.5** (stop the stack, pull, rebuild, update the app):

1. Back up first: `data/`, the `askbar-secrets`, `admin-data`, `admin-secrets` and `audit-data` volumes (see *Backup and recovery*).
2. `docker compose down` (not `-v`), `git pull`, `docker compose up -d --build`. The databases migrate themselves on the first start (the `devices` table becomes `clients`; paired apps and their tokens are kept). <!-- names-ok: the old names are the point of this note -->
3. What moved: the admin is at `https://<hostname>/admin` (the `admin.<hostname>` name and its DNS record are no longer used; before HTTPS was set up 0.5 used `http://127.0.0.1:8081/admin/` on the Docker host; that page is gone, see *First run*). The old token volume `askbar-mgmt` and network `mgmt` are replaced by `server-token` and `server-api`; the admin makes a new server token and writes its hash there on first start, so the old volume can be removed. Data volumes keep their names. CLIProxyAPI now starts with the stack (it idles until you log in). <!-- names-ok: the old names are the point of this note --> If you ran an earlier build of this branch, remove its leftover network first: `docker network rm askbar_setup` (the first-run page now uses `askbar_admin-setup`).
4. Update the Askbar app to 0.5.0. Paired apps keep their tokens, so nothing needs pairing again; an older app says it is too old for the server.
5. If you set any of these in your own compose file, rename them: `ASKBAR_API_ADDR` to `ASKBAR_CLIENT_API_ADDR`, `ASKBAR_MGMT_ADDR` to `ASKBAR_SERVER_API_ADDR`, `ASKBAR_SERVER_MGMT_URL` to `ASKBAR_SERVER_API_URL`, `ASKBAR_MGMT_DIR` to `ASKBAR_SERVER_TOKEN_DIR`, `ASKBAR_MGMT_TOKEN_HASH_FILE` to `ASKBAR_SERVER_TOKEN_HASH_FILE`. <!-- names-ok: the old names are the point of this note -->
6. If anything of yours calls the HTTP APIs: the apps' API is now `/api/client/v1` (`/pair` is `/pairings`, `/whoami` is `/me`), the admin's is `/api/admin/v1`, and every error is a [problem](docs/wire-contract.md#errors) with a stable `error` code.

**Upgrading from 0.2:** the server image now runs as uid 10002 (it was 10001; 10001 is the admin). `chown -R 10002 $DATA_DIR` before starting. The old single-server layout (admin site on :8081, `ASKBAR_UID`/`ASKBAR_GID`, `ASKBAR_API_PORT`, the `tls` profile and Caddy on 8443/9443) is gone, and the first start migrates the database (providers, settings and pairing codes now live in the admin; re-enter them there).

**Set in the admin site** instead of in files: admin password, hostname, Cloudflare token, allowlist, the Let's Encrypt staging switch, the system prompt, display defaults and per-user defaults.

**Compose profiles:** the services (server, admin, Caddy, CLIProxyAPI) are always on. `dev` adds a mock LLM (`tests/e2e/mock_llm.py`).

### Starting and stopping services

The admin site's **Services** tab shows whether the server, the admin, Caddy and CLIProxyAPI answer, and can restart each (it asks first and says what that will interrupt). No service has the Docker socket ([ADR 0005](docs/adr/0005-no-controller-and-self-restart.md)): the server and the admin restart themselves through their own endpoints and Docker's restart policy starts them again (the server is back within a minute; you sign in to the admin again), Caddy has its configuration pushed again, and CLIProxyAPI is restarted by a small supervisor in its image (`askbar-cpa`, [ADR 0037](docs/adr/0037-askbar-cpa-image-and-supervisor.md)), which also restarts it when its config changes or it crashes. A service refuses a restart in its first 30 seconds, and the server and admin also restart themselves if their own health check fails three times in a row. Starting or stopping a service is done from the host (`docker compose stop server`). Only a signed-in person can restart, not an admin API key; each attempt is recorded, and six a minute is the limit. See [SECURITY.md](SECURITY.md) for what this trusts.

**Upgrading from an install with the controller:** `docker compose up -d --remove-orphans` removes the old `askbar-controller` container; the `control` network goes with it. The `control-token` volume stays declared and unused until 1.0.

**Upgrading an install that used `cpa-init` / `cpa-login`:** CLIProxyAPI's config volume was root-owned; the admin (uid 10001) must be able to read and write it once: `docker compose run --rm --no-deps --user root --entrypoint chown admin -R 10001:10001 /cpa-config`, then `docker compose up -d --build`. The admin keeps your existing API key (so the provider and your logins carry on), replaces CLIProxyAPI's control secret and restarts it. If the admin cannot read the config it says so in its log and leaves CLIProxyAPI alone.

**Server environment** (already set in `compose.yaml`; useful without Docker): `ASKBAR_CLIENT_API_ADDR`, `ASKBAR_SERVER_API_ADDR` (one specific address, never `0.0.0.0`), `ASKBAR_SERVER_TOKEN_HASH_FILE`, `ASKBAR_DATA_DIR`, `ASKBAR_SECRETS_DIR` or `ASKBAR_MASTER_KEY_FILE`. **Admin:** `ASKBAR_ADMIN_SOCKET`, `ASKBAR_ADMIN_DATA_DIR`, `ASKBAR_ADMIN_SECRETS_DIR`, `ASKBAR_SERVER_TOKEN_DIR`, `ASKBAR_SERVER_API_URL`, `ASKBAR_CADDY_SOCKET`, `ASKBAR_CADDY_SECRETS_DIR`, `ASKBAR_PUBLIC_URL`.

### Backup and recovery

Four things to back up, **separately**:

- **`data/`** (the server database: conversations, paired clients, and the provider keys *encrypted*) and the **`admin-data`** volume (the admin's database).
- **The `askbar-secrets` volume** (`master.key`) and the **`admin-secrets`** volume (`admin.key`). Keep these somewhere other than your database backups: anyone holding both can read the stored keys. For example: `docker run --rm -v askbar_askbar-secrets:/s -v "$PWD":/b alpine cp /s/master.key /b/master.key`, then store the copy in a password manager.
- Admin site > **Backup** downloads one `.tar` with `server.db` and `admin.db`. It is consistent (`VACUUM INTO`) and leaves out provider API keys, pair codes, the Cloudflare token and the admin password hash, so after restoring it you enter those again.

- **The `audit-data` volume** (`audit.db`, the trace of replies) is **not** in the admin site's Backup `.tar`. Copy the whole volume if you want it (stop the server first, or copy `audit.db`, `audit.db-wal` and `audit.db-shm` together): `docker run --rm -v askbar_audit-data:/a -v "$PWD":/b alpine cp -a /a/. /b/audit-backup/`. A copy holds sealed content only, which is useless without the audit passphrase.

If you restore a raw copy of `data/` that still holds encrypted provider keys onto a **new** `askbar-secrets` volume, the server refuses to start rather than create a new key, and names the volume: restore `master.key` into it first. A new, empty `data/` with a new `askbar-secrets` is a normal fresh install. The server never replaces an existing `master.key`, and there is no key rotation.

## Layout

```
crates/askbar-proto/   request/response and snapshot types shared by the server, admin and client
crates/askbar-service/ what every service shares: per-link token checks, GET /status, the not-configured gate
services/server/       the server: client API, server API, database, chat/LLM streaming, secrets
services/admin/        the admin service: admin site (src/ui/), admin.db, snapshot push, Caddy configuration (unix only)
services/cpa-supervisor/ runs CLIProxyAPI in the askbar-cpa image and restarts it on a config change, a request or a crash
clients/desktop/       the desktop client, its own Cargo workspace and package.json:
  src-tauri/           Cargo package askbar-client, program askbar: window, hotkey, tray, pairing
  ui/                  the popup: index.html, style.css, main.js, markdown.js (no build step)
  tests/               markdown and UI tests for the popup, icon generator
deploy/caddy/          Caddy's bootstrap config
deploy/cpa/            notes on CLIProxyAPI
compose.yaml           server, admin, caddy, cpa (CLIProxyAPI), plus the dev profile
Dockerfile             one target per service: `server`, `admin`, `caddy` (Caddy with the Cloudflare DNS module) and `cpa` (the pinned CLIProxyAPI with our supervisor)
scripts/               smoke.sh (end-to-end), check-isolation.sh (container isolation checks) and check-docs.sh
docs/                  architecture.md, wire-contract.md, adr/, style/, openapi/ (client, server, admin), refactor-log.md
tests/e2e/             the admin-site browser test and the mock LLM server
```

The target layout is in [ADR 0033](docs/adr/0033-repository-layout.md).

## Tests

```sh
npm test                         # everything below except smoke and isolation, plus the names and link checks
cargo test --workspace           # proto, server and admin (needs python3 for the mock LLM)
npm --prefix clients/desktop test   # the app: its cargo tests, markdown, and the UI against a mocked backend
cargo build --workspace && node tests/e2e/admin.test.js   # the admin site in a real browser against the real server and admin
scripts/smoke.sh                 # server + admin + mock LLM: setup, pairing, chat, search
docker compose up -d && scripts/check-isolation.sh    # against a throwaway stack
python3 tests/e2e/mock_llm.py 8765   # fake OpenAI-compatible server to try the app without a key
```

The UI and admin tests need `npm install && npm --prefix clients/desktop install && npx playwright install chromium`.

## Verified, and not

Verified on Linux (X11) by driving the real binary: hotkey show/hide, second-launch toggle, typing, streaming, Esc-to-stop (the server sees the disconnect and the partial reply is saved), deleting through real clicks, clipboard copy and paste, a bad hotkey, and hide-on-click-away (these were checked before the 0.3.0 split; the 0.3.0 client has been build- and test-verified, not driven by hand).

Verified for 0.3.0 in containers: all three images build and report healthy; `scripts/check-isolation.sh` passes (server port token and fail-closed behaviour, network separation, what each container can read, the allowlist rules in Caddy's loaded configuration, secret file modes, no admin code in the server image, backups free of secrets); the admin applies configuration to Caddy through its unix socket.

Not verified: **Windows and macOS** runtime behaviour (`.github/workflows/build.yml`, run manually with *Run workflow*, builds and tests them), **Wayland**, a real certificate issued by this layout (the Let's Encrypt/Cloudflare path worked in 0.2 and the Caddy module is unchanged), a **desktop client paired to a 0.3.0 stack by a person**, and the live Anthropic and OpenAI APIs (both protocols are tested against a mock that follows their streaming formats). On macOS the dock icon is hidden by default; if Cmd+C or Cmd+V misbehave, set `ui.macos_hide_dock_icon: false`.

## Not built yet

- **Projects**: group conversations, move them between projects (the database already has the tables).
- **Deployment on a NAS, scheduled backups, VPN access, arm64 images.**
- **Self-improving memory**: after a conversation, a cheap model call proposes add/update/delete operations on a list of durable facts; the top matches are injected into the next system prompt; a periodic pass merges duplicates and drops stale entries; a view lets you edit or delete anything it learned. The schema has a migration slot ready.
- Renaming conversations.

in collaboration with thewuurbot

## License

Askbar is licensed under the [GNU Affero General Public License v3.0 or later](LICENSE) (AGPL-3.0-or-later).

**Source code:** <https://github.com/Thewuuryar/askbar>. If you run a modified server for other people, section 13 of the AGPL requires offering them the corresponding source; the admin page links here for that reason. The Caddy and CLIProxyAPI containers are separate programs with their own licenses.
