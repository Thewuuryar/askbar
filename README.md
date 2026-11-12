# Askbar

A Spotlight-style AI popup. Press a hotkey, a bar appears in the upper-middle of the screen, type a question, read the answer, press Esc. Your own server holds the conversations and the provider keys; the desktop app is a thin client that pairs with it.

Askbar is an open-source project for personal hardware, with no support or uptime guarantee.

Everything else (how it fits together, configuration, HTTPS, the audit trail, upgrades, backups, tests) is in the **[Askbar guide](docs/guide.md)**. The design is in [docs/architecture.md](docs/architecture.md) and the [ADRs](docs/adr/README.md).

## V1 features

Version 1.0 is the scope of [ADR 0002](docs/adr/0002-v1-scope-limits-and-ship-criteria.md). Everything below is built unless marked *planned* or *deferrable*.

- **Conversation only.** Text in, text out: no tools, no shell, no file access.
- **A desktop popup** for macOS, Windows and Linux, opened with a global hotkey.
- **Any model provider:** Anthropic, anything OpenAI-compatible (OpenAI, OpenRouter, Ollama, LM Studio, vLLM, ...), or a Claude or ChatGPT subscription through CLIProxyAPI.
- **Replies run on the server.** Close the window or lose the network and the reply carries on; reopen and it is there. Messages can wait their turn, and Stop keeps the text so far.
- **History and search**, in a personal space per user, each kept in its own database file. *Deferrable:* shared spaces for a family ([ADR 0009](docs/adr/0009-spaces-and-storage.md)).
- **Users and pairing keys:** the admin makes users, and a pairing key pairs an app over HTTPS, trusting exactly this server's certificate. A paired app can pair another.
- **An admin site** at `/admin`: first-run setup, providers, users, settings, token budgets and a kill switch.
- **An audit trail** of every reply, with prompts and replies sealed by a passphrase only the owner knows.
- **HTTPS from the first start** with the install's own certificate, then Let's Encrypt for your own host name.
- **Hardened, separated containers:** only Caddy publishes ports, and no service has the Docker socket. Each link between services has its own token and network. A separate **auditor** keeps the audit store, and a **`model-gateway`** alone holds the provider keys and the approved model list ([architecture](docs/architecture.md#components-at-a-glance)).
- **Alerts** by ntfy or email, from a small `alerter` container that also checks for new releases.
- **Encrypted backups,** by hand or on a schedule, and a restore.
- *Deferrable:* memory and an iOS app.

## Quick start: deploy the server

You need Docker with Compose.

```sh
cp .env.example .env
mkdir -p data
docker compose up -d --build
```

1. Open **https://localhost/admin** on the machine that runs Docker, or `https://<its address>/admin` from another machine on your network, and accept the certificate warning (the install uses its own certificate until you set a host name).
2. Sign in with the published login, **`admin` / `admin`**, and change the password. Every page refuses until you do.
3. Choose the **audit passphrase**: at least 20 characters. It cannot be recovered, so write it down.
4. **Providers** > add one and press **Test**, or connect a Claude or ChatGPT account on **Services**.
5. **Clients** > add a user, then make a pairing key for them.

To stop the stack, or before rebuilding after an update: `docker compose --profile dev down --remove-orphans`. For a host name with a real certificate, the allowlist, backups and upgrades, see the [guide's server section](docs/guide.md#server).

## The desktop app

### Build and run

You need [Rust](https://rustup.rs), Node 18+, and your platform's WebView:

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

### Pair

Tray icon > **Pair with server...**, then paste a pairing key from the admin site (**Clients**), or from an app you already paired (tray icon > **Pair another app...**). A key works once and expires after 10 minutes. The app keeps its token in the OS keychain. If the server can't be reached it says "server offline"; there is no offline mode.

### Use

| Key | Action |
|---|---|
| your hotkey (`Ctrl+Shift+Space` by default) | show / hide |
| Enter / Shift+Enter | send / new line |
| Esc | stop a reply that is streaming; otherwise hide |
| Ctrl+H | history panel |
| Ctrl+N | new conversation |
| Ctrl+Shift+C | copy the last reply |

Ctrl is used on macOS too. On Wayland, bind a shortcut to the command `askbar` instead. The hotkey, `client.yaml`, the keychain prompt on macOS and the rest are in the [guide's desktop app section](docs/guide.md#the-desktop-app).

## License

[AGPL-3.0-or-later](LICENSE). Source: <https://github.com/Thewuuryar/askbar>; see the [guide](docs/guide.md#license). The admin site shows the same notice and source link, so anyone using a server over the network can find the source.
