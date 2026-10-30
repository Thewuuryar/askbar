# Askbar

A Spotlight-style AI popup. Press a hotkey, a bar appears in the upper-middle of the screen, type a question, read the answer, press Esc.

- **Conversation only.** The model gets text in and returns text out. There are no tools, no shell, no file access, and the window is granted no filesystem or network permissions (`src-tauri/capabilities/default.json`). Only the Rust side talks to your provider.
- **Any provider.** Anthropic, or anything that speaks the OpenAI `/chat/completions` protocol (OpenAI, OpenRouter, Ollama, LM Studio, vLLM, Groq, ...).
- **History** you can toggle (Ctrl+H), reopen, and delete. Stored in a local SQLite file.
- **Copy in and out.** Paste anything into the bar. Copy a whole reply, a single code block, or the last reply (Ctrl+Shift+C).
- **Settings in one YAML file.**

Askbar works in two ways:

- **Standalone** (no setup): history and provider settings live on your machine, as described under [Run it](#run-it) and [Config](#config).
- **Client and server**: a small server holds your history, provider keys and settings; the desktop app is a thin client that pairs with it. Several devices share one history, and API keys never touch the clients. See [Server](#server).

## Run it

Standalone mode. Prerequisites: [Rust](https://rustup.rs), Node 18+, and your platform's WebView.

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

The first launch writes a commented config file and shows a welcome message with its path. Add your API key, then ask.

## Config

`~/.config/askbar/config.yaml` (Linux), `~/Library/Application Support/askbar/config.yaml` (macOS), `%APPDATA%\askbar\config.yaml` (Windows). Override with `ASKBAR_CONFIG=/path/to/file.yaml`. Tray icon → **Open config file**.

```yaml
hotkey: "Ctrl+Shift+Space"
active_provider: anthropic

providers:
  - name: anthropic
    type: anthropic            # anthropic | openai
    base_url: https://api.anthropic.com
    api_key_env: ANTHROPIC_API_KEY   # or put the key in api_key
    model: claude-sonnet-5-5
  - name: ollama
    type: openai
    base_url: http://localhost:11434/v1
    model: llama3.2
```

Keys in `api_key` are stored as plain text in that file; `api_key_env` reads an environment variable instead (an OS keychain option is a natural next step).

Provider, model, key and prompt edits apply on your next message. `hotkey` and `ui` changes apply from tray icon → **Reload config**. A YAML mistake is reported in the window with the file and line.

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

Your data: `history.db` in the platform data directory (`~/.local/share/askbar/` on Linux). Override with `ASKBAR_DB`.

## Hotkeys by platform

`Super+A` is accepted (`hotkey: "Super+A"`) and works on X11, but Windows reserves it for Quick Settings and GNOME for the app grid, so the default is `Ctrl+Shift+Space`. If a hotkey can't be registered, the window opens with the reason.

**Wayland** does not allow ordinary apps to register global hotkeys. Bind a shortcut in your desktop's keyboard settings to the command `askbar`. Running it while Askbar is already running toggles the window instead of starting a second copy. This also works on any platform.

## Server

```
 desktop app (Tauri)  --HTTPS, bearer token-->  Caddy (profile tls) --> askbar-server --> your providers
 desktop app #2       -------------------------/                       |  SQLite (/data)  \-> CLIProxyAPI (profile cpa)
 browser (admin page) --HTTPS, session cookie-->/                       master key (/secrets, own volume)
```

- `askbar-server` serves two listeners: the **API** (port 8080 in the container) for paired devices, and the **admin site** (8081). The API accepts only `Authorization: Bearer <device token>` and never reads cookies; the admin site accepts only its session cookie and rejects device tokens.
- Chat replies stream to clients as Server-Sent Events (`meta`, `delta`, `done`, `error`); provider output is never passed through.
- Device tokens are random, shown once, and stored as SHA-256 hashes. Provider API keys are encrypted at rest (ChaCha20-Poly1305) with a master key and are never returned by any endpoint.
- A client refuses `http://` unless the server is on this machine, or `allow_insecure: true` is set in `client.yaml`.

### Quick start (this machine, plain HTTP)

Needs Docker with Compose.

```sh
cp .env.example .env
docker compose up -d --build
```

Open <http://localhost:8081/admin/> (the admin site is published on `127.0.0.1` only) and finish **first-run setup**:

1. Choose the admin password (first visitor wins, which is safe because the port is loopback-only).
2. **Providers** > add one (Anthropic, OpenAI-compatible, or the CLIProxyAPI preset) and press **Test**.
3. **Devices** > generate a pairing string.

The server creates its master key by itself on its first start and keeps it in a separate Docker volume (`askbar-secrets`), so copies and backups of the data never contain it. **Back that volume up too, separately from `data/`** (see Backup and recovery).

No secret ever goes in `.env`, so nothing sensitive can be committed by accident. The password and the Cloudflare token are entered on the admin page.

### Pair a desktop app

In the app, tray icon > **Pair with server...**, paste the pairing string. The one-time code expires after 10 minutes; the app exchanges it for a per-device token that is stored in the OS keychain, not in a file. Revoke a device from the **Devices** tab and it shows "token rejected".

`client.yaml` (next to `config.yaml`; its presence switches the app to the server) holds only:

```yaml
server_url: https://askbar.example.com:8443
hotkey: "Ctrl+Shift+Space"
active_provider: anthropic     # empty = the server's first provider
allow_insecure: false
ui:                            # optional; anything left out follows the server's defaults
  theme: dark
  width: 640
```

`ASKBAR_CLIENT_CONFIG` points at a different file, which is also how you run a second profile on one machine. If the server can't be reached the app says "server offline"; there is no offline mode.

### HTTPS with your own hostname (Caddy, profile `tls`)

Certificates come from Let's Encrypt using a DNS challenge on Cloudflare, so no inbound port 80/443 is needed and the hostname may point at a private address.

1. In Cloudflare, add an `A` record for your hostname (for a private server, its LAN address), and create an API token with **Zone:DNS:Edit** and **Zone:Zone:Read**.
2. `docker compose --profile tls up -d --build`
3. Admin page > **Network**: tick HTTPS, enter the hostname, the Cloudflare token, and the networks allowed to open the admin site (for example `192.168.1.0/24`). Leave **staging** on for the first try, then turn it off for a trusted certificate.

The API is then at `https://<hostname>:8443` and the admin site at `https://<hostname>:9443` (restricted to the networks you listed). The token is stored encrypted and written to a file that only Caddy can read, and the server tells Caddy to reload. If you reach the admin site from the same machine through Docker, the connection can appear to come from the Docker gateway (for example `172.x`), so include that range in the allowed networks.

### CLIProxyAPI (profile `cpa`)

Lets a Claude or ChatGPT subscription act as an OpenAI-compatible provider. See [cpa/README.md](cpa/README.md) for the one-time login. Its API is reachable only from the server, never published.

### Configuration reference

**`.env`** (read by Compose; nothing secret):

| Variable | Default | Meaning |
|---|---|---|
| `ASKBAR_API_PORT` | 8080 | host port for the API, plain HTTP |
| `ASKBAR_ADMIN_PORT` | 8081 | host port for the admin site, bound to `127.0.0.1` |
| `ASKBAR_TLS_API_PORT` / `ASKBAR_TLS_ADMIN_PORT` | 8443 / 9443 | host ports of the HTTPS front end |
| `ASKBAR_PUBLIC_URL` | `http://localhost:$ASKBAR_API_PORT` | address put in pairing strings while HTTPS is off |
| `DATA_DIR` | `./data` | where the database lives on the host |
| `ASKBAR_UID` / `ASKBAR_GID` | 10001 | user the server runs as; `DATA_DIR` must be writable by it |

**Set on the admin page** instead of in files: admin password, hostname, Cloudflare token, allowed admin networks, the Let's Encrypt staging switch, the system prompt, and display defaults.

**Compose profiles:** the `server` is always on. `dev` adds a mock LLM (`tests/mock_llm.py`), `tls` adds Caddy, `cpa` adds CLIProxyAPI.

**Server environment** (already set in `compose.yaml`; useful for running it without Docker): `ASKBAR_API_ADDR`, `ASKBAR_ADMIN_ADDR`, `ASKBAR_DATA_DIR`, `ASKBAR_SECRETS_DIR` or `ASKBAR_MASTER_KEY_FILE`, `ASKBAR_CADDY_ADMIN`, `ASKBAR_CADDY_SECRETS_DIR`.

### Backup and recovery

There are two things to back up, **separately**:

- **`data/`** (the database: conversations, settings, paired devices, and the provider keys *encrypted*). Admin page > **Backup** downloads a consistent snapshot (`VACUUM INTO`) of it that leaves out provider API keys, the Cloudflare token and the admin password, so after restoring that snapshot you enter those again.
- **The `askbar-secrets` volume** (`master.key`). Keep it somewhere other than your `data/` backups: anyone holding both can read the stored provider keys. For example: `docker run --rm -v askbar-secrets:/s -v "$PWD":/b alpine cp /s/master.key /b/master.key`, then store the copy in a password manager.

If you restore a raw copy of `data/` that still holds encrypted provider keys onto a **new** `askbar-secrets` volume, the server refuses to start rather than create a new key, and names the volume: restore `master.key` into it first. A new, empty `data/` with a new `askbar-secrets` is a normal fresh install. The server never replaces an existing `master.key`, and there is no key rotation.

## Layout

```
core/            config.rs (YAML), db.rs (SQLite + migrations), provider.rs (SSE streaming)
                 no GUI dependencies; unit + mock-server tests
askbar-proto/    request/response types shared by the server and the client
askbar-server/   the server: API, admin site (src/admin_ui), secrets, Caddy integration
src-tauri/       the desktop client: window, hotkey, tray; Local (standalone) and Remote (server) backends
ui/              index.html, style.css, main.js, markdown.js (no build step)
caddy/, cpa/     Caddy bootstrap config; CLIProxyAPI setup script and login notes
compose.yaml     server, plus the dev, tls and cpa profiles
tests/           markdown, UI and admin-site browser tests, a mock LLM server, icon generator
```

## Tests

```sh
cargo test --workspace           # core, server, protocol and client (needs python3 for the mock LLM)
node tests/markdown.test.js
node tests/ui.test.js            # desktop UI against a mocked backend
cargo build -p askbar-server && node tests/admin.test.js   # the admin site in a real browser against the real server
python3 tests/mock_llm.py 8765   # fake OpenAI-compatible server to try the app without a key
```

The UI and admin tests need `npm install && npx playwright install chromium`.

## Verified, and not

Verified on Linux (X11) by driving the real binary: hotkey show/hide, second-launch toggle, typing, streaming, Esc-to-stop (the server sees the disconnect and the partial reply is saved), deleting through real clicks with the database checked, clipboard copy and paste, first-run config, a bad hotkey, and hide-on-click-away.

Server side, additionally verified: first-run setup, pairing and revocation, chat streaming with client disconnects, the Caddy configuration (validated by Caddy itself, and a staging certificate issued over a Cloudflare DNS challenge), and a CLIProxyAPI-backed provider.

Not verified: **Windows and macOS** runtime behaviour (`.github/workflows/build.yml`, run manually with *Run workflow*, builds and tests them), **Wayland**, and the live Anthropic and OpenAI APIs (both protocols are tested against a mock that follows their streaming formats). On macOS the dock icon is hidden by default; if Cmd+C or Cmd+V misbehave, set `ui.macos_hide_dock_icon: false`.

## Not built yet

- **Projects**: group conversations, move them between projects (the database already has the tables).
- **Deployment on a NAS, scheduled backups, VPN access, arm64 images.**
- **Self-improving memory**: after a conversation, a cheap model call proposes add/update/delete operations on a list of durable facts; the top matches are injected into the next system prompt; a periodic pass merges duplicates and drops stale entries; a view lets you edit or delete anything it learned. The schema has a migration slot ready.
- Conversation search; renaming.

## License

Askbar is licensed under the [GNU Affero General Public License v3.0 or later](LICENSE) (AGPL-3.0-or-later).

**Source code:** <https://github.com/Thewuuryar/askbar>. If you run a modified server for other people, section 13 of the AGPL requires offering them the corresponding source; the admin page links here for that reason. The Caddy and CLIProxyAPI containers are separate programs with their own licenses.
