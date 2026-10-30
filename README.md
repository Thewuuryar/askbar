# Askbar

A Spotlight-style AI popup. Press a hotkey, a bar appears in the upper-middle of the screen, type a question, read the answer, press Esc.

- **Conversation only.** The model gets text in and returns text out. There are no tools, no shell, no file access, and the window is granted no filesystem or network permissions (`src-tauri/capabilities/default.json`). Only the Rust side talks to your provider.
- **Any provider.** Anthropic, or anything that speaks the OpenAI `/chat/completions` protocol (OpenAI, OpenRouter, Ollama, LM Studio, vLLM, Groq, ...).
- **History** you can toggle (Ctrl+H), reopen and delete. Stored on your server, shared by every device you pair.
- **Copy in and out.** Paste anything into the bar. Copy a whole reply, a single code block, or the last reply (Ctrl+Shift+C).
- **Keys stay on the server.** The desktop app is a thin client; provider API keys never reach it.

Askbar is three programs:

- **Desktop client** (Tauri): the popup. It pairs with one server and holds only its address, a hotkey and a device token (in the OS keychain).
- **`askbar-server`**: the device API. It stores conversations, runs the chat against your providers and keeps provider keys encrypted. It has no web UI and no login.
- **`askbar-admin`**: the admin site. It owns all configuration (providers, system prompt, display defaults, pairing codes, devices, network and HTTPS settings) and pushes it to the server over a private management API.

There is no standalone mode: the client always talks to a server.

```
 desktop app --HTTPS, bearer token--> Caddy :443 --> askbar-server :8080 --> your providers
 browser     --HTTPS, session cookie-> Caddy :443 --unix socket--> askbar-admin
                                                                      |  management API (token, own network)
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
npm install
npm run dev        # run it
npm run build      # installers land in target/release/bundle/
```

Until the app is paired it shows how to pair. Tray icon > **Pair with server...**, paste the pairing string from the admin site (Devices). The one-time code expires after 10 minutes; the app exchanges it for a per-device token stored in the OS keychain, not in a file. Revoke a device in the admin site and the app shows "token rejected". If the server can't be reached the app says "server offline"; there is no offline mode.

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
| `askbar-server` | uid 10002 | `edge`, `mgmt` | nothing |
| `askbar-admin` | uid 10001 | `mgmt` only | nothing (a unix socket) |

`mgmt` is an `internal` network shared by the admin and the server's management API (172.30.0.2:8082, fixed in `compose.yaml`). Caddy is the only thing the outside world can reach.

- The **server** serves the device API, which accepts only `Authorization: Bearer <device token>`. Chat replies stream as Server-Sent Events (`meta`, `delta`, `done`, `error`). Device tokens are random, shown once, and stored as SHA-256 hashes. See [docs/wire-contract.md](docs/wire-contract.md).
- The **admin** listens on a unix socket only. Caddy proxies `admin.<hostname>` to it, and the admin owns `admin.db`. Every change is made in `admin.db` and pushed to the server as one snapshot; if the server is down the change is refused ("The server is offline. Nothing was changed.") rather than half applied. With the server stopped the admin still loads and shows an offline banner.
- The **management API** requires a bearer token. The admin keeps the token in its own secrets volume (mode 600); the server holds only its SHA-256 (`token.sha256`, read per request, compared in constant time, and the API refuses everything when the file is missing).
- **Provider API keys** are encrypted at rest (ChaCha20-Poly1305) with a master key that the server creates on its first start in the `askbar-secrets` volume. They are sent to the server by the admin, stored only in `server.db`, and never returned by any endpoint. The Cloudflare token is encrypted in `admin.db` with the admin's own key (`admin-secrets`), separate from the server's, and written to a file only Caddy reads.

### First run

> **Open item:** the admin has no TCP port, and Caddy only serves `admin.<hostname>` once a hostname has been set in the admin. How a first-time visitor reaches the admin before HTTPS exists is not settled yet, and this README does not invent a way. `scripts/smoke.sh` and `tests/admin.test.js` drive the admin over its unix socket.

Once you can reach the admin site:

1. Choose the admin password (the first visit wins).
2. **Providers** > add one (Anthropic, OpenAI-compatible, or the CLIProxyAPI preset) and press **Test**.
3. **Network** > hostname, Cloudflare token and, if you want it, the allowlist (see below).
4. **Devices** > generate a pairing string.

No secret ever goes in `.env`, so nothing sensitive can be committed by accident.

### HTTPS with your own hostname

Certificates come from Let's Encrypt using a DNS challenge on Cloudflare, so the hostname may point at a private address.

1. In Cloudflare, add an `A` record for your hostname (for a private server, its LAN address), and create an API token with **Zone:DNS:Edit** and **Zone:Zone:Read**.
2. Admin site > **Network**: tick HTTPS, enter the hostname and the Cloudflare token. Leave **staging** on for the first try, then turn it off for a trusted certificate.
3. The API is at `https://<hostname>` and the admin site at `https://admin.<hostname>`.

The admin allowlist is **off by default**: anyone who can reach `admin.<hostname>` can try to log in (login is rate limited). Tick **Only let these addresses open the admin site over HTTPS** and list addresses or CIDRs (for example `192.168.1.0/24`) to make Caddy drop every other address before it reaches the admin. If you reach the admin from the same machine through Docker, the connection can appear to come from the Docker gateway (for example `172.x`), so include that range.

### CLIProxyAPI (profile `cpa`)

Lets a Claude or ChatGPT subscription act as an OpenAI-compatible provider. See [cpa/README.md](cpa/README.md) for the one-time login. It joins the `edge` network only and its API is never published.

### Configuration reference

**`.env`** (read by Compose; nothing secret):

| Variable | Default | Meaning |
|---|---|---|
| `ASKBAR_PUBLIC_URL` | empty | address put in pairing strings until HTTPS is configured; with HTTPS on it is `https://<hostname>` |
| `DATA_DIR` | `./data` | where the server's database lives on the host; must be writable by **uid 10002** |

**Upgrading from 0.2:** the server image now runs as uid 10002 (it was 10001; 10001 is the admin). `chown -R 10002 $DATA_DIR` before starting. The old single-server layout (admin site on :8081, `ASKBAR_UID`/`ASKBAR_GID`, `ASKBAR_API_PORT`, the `tls` profile and Caddy on 8443/9443) is gone, and the first start migrates the database (providers, settings and pairing codes now live in the admin; re-enter them there).

**Set in the admin site** instead of in files: admin password, hostname, Cloudflare token, allowlist, the Let's Encrypt staging switch, the system prompt, display defaults and per-user defaults.

**Compose profiles:** the three services are always on. `dev` adds a mock LLM (`tests/mock_llm.py`), `cpa` adds CLIProxyAPI.

**Server environment** (already set in `compose.yaml`; useful without Docker): `ASKBAR_API_ADDR`, `ASKBAR_MGMT_ADDR` (one specific address, never `0.0.0.0`), `ASKBAR_MGMT_TOKEN_HASH_FILE`, `ASKBAR_DATA_DIR`, `ASKBAR_SECRETS_DIR` or `ASKBAR_MASTER_KEY_FILE`. **Admin:** `ASKBAR_ADMIN_SOCKET`, `ASKBAR_ADMIN_DATA_DIR`, `ASKBAR_ADMIN_SECRETS_DIR`, `ASKBAR_MGMT_DIR`, `ASKBAR_SERVER_MGMT_URL`, `ASKBAR_CADDY_SOCKET`, `ASKBAR_CADDY_SECRETS_DIR`, `ASKBAR_PUBLIC_URL`.

### Backup and recovery

Three things to back up, **separately**:

- **`data/`** (the server database: conversations, paired devices, and the provider keys *encrypted*) and the **`admin-data`** volume (the admin's database).
- **The `askbar-secrets` volume** (`master.key`) and the **`admin-secrets`** volume (`admin.key`). Keep these somewhere other than your database backups: anyone holding both can read the stored keys. For example: `docker run --rm -v askbar_askbar-secrets:/s -v "$PWD":/b alpine cp /s/master.key /b/master.key`, then store the copy in a password manager.
- Admin site > **Backup** downloads one `.tar` with `server.db` and `admin.db`. It is consistent (`VACUUM INTO`) and leaves out provider API keys, pair codes, the Cloudflare token and the admin password hash, so after restoring it you enter those again.

If you restore a raw copy of `data/` that still holds encrypted provider keys onto a **new** `askbar-secrets` volume, the server refuses to start rather than create a new key, and names the volume: restore `master.key` into it first. A new, empty `data/` with a new `askbar-secrets` is a normal fresh install. The server never replaces an existing `master.key`, and there is no key rotation.

## Layout

```
askbar-proto/    request/response and snapshot types shared by the server, admin and client
askbar-server/   the server: device API, management API, database, chat/LLM streaming, secrets
askbar-admin/    the admin service: admin site (ui/), admin.db, snapshot push, Caddy configuration (unix only)
src-tauri/       the desktop client: window, hotkey, tray, pairing
ui/              the popup: index.html, style.css, main.js, markdown.js (no build step)
caddy/, cpa/     Caddy bootstrap config; CLIProxyAPI setup script and login notes
compose.yaml     server, admin, caddy, plus the dev and cpa profiles
Dockerfile       two targets: `admin` and `server`;  Dockerfile.caddy: Caddy with the Cloudflare DNS module
scripts/         smoke.sh (end-to-end), check-isolation.sh (container isolation checks)
docs/            wire-contract.md, refactor-log.md
tests/           markdown, UI and admin-site browser tests, a mock LLM server, icon generator
```

## Tests

```sh
cargo test --workspace           # proto, server, admin and client (needs python3 for the mock LLM)
node tests/markdown.test.js
node tests/ui.test.js            # desktop UI against a mocked backend
cargo build --workspace && node tests/admin.test.js   # the admin site in a real browser against the real server and admin
scripts/smoke.sh                 # server + admin + mock LLM: setup, pairing, chat, search
docker compose up -d && scripts/check-isolation.sh    # against a throwaway stack
python3 tests/mock_llm.py 8765   # fake OpenAI-compatible server to try the app without a key
```

The UI and admin tests need `npm install && npx playwright install chromium`.

## Verified, and not

Verified on Linux (X11) by driving the real binary: hotkey show/hide, second-launch toggle, typing, streaming, Esc-to-stop (the server sees the disconnect and the partial reply is saved), deleting through real clicks, clipboard copy and paste, a bad hotkey, and hide-on-click-away (these were checked before the 0.3.0 split; the 0.3.0 client has been build- and test-verified, not driven by hand).

Verified for 0.3.0 in containers: all three images build and report healthy; `scripts/check-isolation.sh` passes (management port token and fail-closed behaviour, network separation, what each container can read, the allowlist rules in Caddy's loaded configuration, secret file modes, no admin code in the server image, backups free of secrets); the admin applies configuration to Caddy through its unix socket.

Not verified: **Windows and macOS** runtime behaviour (`.github/workflows/build.yml`, run manually with *Run workflow*, builds and tests them), **Wayland**, a real certificate issued by this layout (the Let's Encrypt/Cloudflare path worked in 0.2 and the Caddy module is unchanged), a **desktop client paired to a 0.3.0 stack by a person**, and the live Anthropic and OpenAI APIs (both protocols are tested against a mock that follows their streaming formats). On macOS the dock icon is hidden by default; if Cmd+C or Cmd+V misbehave, set `ui.macos_hide_dock_icon: false`.

## Not built yet

- **Projects**: group conversations, move them between projects (the database already has the tables).
- **Deployment on a NAS, scheduled backups, VPN access, arm64 images.**
- **Self-improving memory**: after a conversation, a cheap model call proposes add/update/delete operations on a list of durable facts; the top matches are injected into the next system prompt; a periodic pass merges duplicates and drops stale entries; a view lets you edit or delete anything it learned. The schema has a migration slot ready.
- Renaming conversations.

## License

Askbar is licensed under the [GNU Affero General Public License v3.0 or later](LICENSE) (AGPL-3.0-or-later).

**Source code:** <https://github.com/Thewuuryar/askbar>. If you run a modified server for other people, section 13 of the AGPL requires offering them the corresponding source; the admin page links here for that reason. The Caddy and CLIProxyAPI containers are separate programs with their own licenses.
