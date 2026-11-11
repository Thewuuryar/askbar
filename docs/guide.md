# Askbar guide

The details behind the [README](../README.md): how Askbar is put together, how to run, configure, upgrade and back up the server, the desktop app in full, the tests, and what is and is not verified.

This page describes the code on `main`. The accepted v1 target differs: see [architecture.md](architecture.md) and the [ADRs](adr/README.md). Where they disagree, build to the ADR.

## What Askbar does today

- **Conversation only.** The model gets text in and returns text out. There are no tools, no shell, no file access, and the window is granted no filesystem or network permissions (`clients/desktop/src-tauri/capabilities/default.json`). Only the Rust side talks to your provider.
- **Any provider.** Anthropic, or anything that speaks the OpenAI `/chat/completions` protocol (OpenAI, OpenRouter, Ollama, LM Studio, vLLM, Groq, ...).
- **History** you can toggle (Ctrl+H), reopen and delete. Stored on your server, shared by every app you pair.
- **Copy in and out.** Paste anything into the bar. Copy a whole reply, a single code block, or the last reply (Ctrl+Shift+C).
- **Replies run on the server.** Close the window or lose the connection and the reply keeps going; reopen the app and it is there.
- **Keys stay off the desktop.** The desktop app is a thin client; provider API keys never reach it, or the server.

## How it fits together

Askbar is three programs:

- **Desktop client** (Tauri): the popup. It pairs with one server and holds only its address, a hotkey and a client token (in the OS keychain).
- **`askbar-server`**: the client API. It stores conversations and asks `model-gateway` for each reply; it holds no provider key and has no route to the internet. It has no web UI and no login.
- **`askbar-model-gateway`**: holds the provider keys in memory and calls the providers ([ADR 0054](adr/0054-model-gateway.md)).
- **`askbar-admin`**: the admin site. It owns all configuration (providers, system prompt, display defaults, pairing codes, clients, network and HTTPS settings, limits and the kill switch) and pushes it to the server over a private server API. Its **Audit** page shows what the server recorded about every reply.

There is no standalone mode: the client always talks to a server.

```
 desktop app --HTTPS, bearer token--> Caddy :443 --> askbar-server :8080 --> your providers
 browser     --HTTPS, session cookie-> Caddy :443 --unix socket--> askbar-admin
                                                                      |  server API (token, own network)
                                                                      +--> askbar-server
```

How the pieces fit, and why, is in [architecture.md](architecture.md); the HTTP APIs are described in [openapi/](openapi/) (client, server, admin); contributing is in [CONTRIBUTING.md](../CONTRIBUTING.md) and security in [SECURITY.md](../SECURITY.md).

## The desktop app

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

To stop it, or before rebuilding after an update, use `docker compose --profile dev down --remove-orphans`. With `--profile dev`, the mock LLM is removed as well; without it, a dev container left behind can point at a network that no longer exists.

That starts these containers, with `bootstrap` (no network, exits when it has made the tokens) before them:

| Container | Runs as | Network | Published |
|---|---|---|---|
| `askbar-caddy` | root (group 10001) | `edge`, `client-api`, `audit-caddy` | **443, 80** |
| `askbar-server` | uid 10002 | `client-api`, `server-api`, `gateway-server`, `audit-server` | nothing |
| `askbar-admin` | uid 10001 | `server-api`, `cpa-admin`, `audit-admin`, `gateway-admin` | nothing (a unix socket) |
| `askbar-auditor` | uid 10003 | `audit-admin`, `audit-caddy`, `audit-model-gateway`, `audit-server` only | nothing |
| `askbar-model-gateway` | uid 10004 | `gateway-server`, `gateway-admin`, `audit-model-gateway`, `cpa-gateway`, `outbound` | nothing |

`server-api` is an `internal` network shared by the admin and the server's server API. No address is fixed: the server has the alias `server-api-host` on that network only, listens on the address it resolves to (port 8082), and the admin calls `http://server-api-host:8082`. So the server API answers on `server-api` and nowhere else. Caddy is the only thing the outside world can reach.

### Docker networks

Askbar gives each link between two services its own network, so a service reaches only the services it has to ([architecture.md](architecture.md#links)). The stack has twelve networks today (`edge`, `client-api`, `server-api`, `cpa-admin`, `cpa-gateway`, `audit-admin`, `audit-caddy`, `audit-model-gateway`, `audit-server`, `gateway-admin`, `gateway-server` and `outbound`) and a few more once the alerter and the other callers get theirs. Each network takes a subnet from Docker's address pools, and with Docker's default settings a host has about 30 to share between all its Compose projects. When they run out, `docker compose up` fails with:

```text
could not find an available, non-overlapping IPv4 address pool among the defaults to assign to the network
```

The fix is to give Docker smaller subnets, so the same address range holds many more networks. On Linux, add this to `/etc/docker/daemon.json` (create the file if it does not exist) and restart Docker with `sudo systemctl restart docker`. With Docker Desktop, paste it under **Settings > Docker Engine** and press **Apply & restart**:

```json
{
  "default-address-pools": [
    { "base": "172.16.0.0/12", "size": 24 }
  ]
}
```

That gives 4,096 networks of 254 addresses each, plenty for Askbar and any other projects on the host. Pick a `base` your home network or VPN does not use. If yours uses part of `172.16.0.0/12`, use a range such as `10.200.0.0/16` (256 networks) instead. Networks that already exist keep their subnets. Run `docker compose down` and then `docker compose up -d` to recreate Askbar's from the new pools.

New Askbar networks never pin a subnet; Docker assigns them from these pools.

Every container has a read-only root filesystem, no Linux capabilities (Caddy keeps only the one to bind 80 and 443), no privilege gain, and limits on processes and memory; it writes only to its own volumes and a private `/tmp` ([ADR 0032](adr/0032-container-hardening.md)). Caddy, though root, reads the admin's certificate key and Cloudflare token (mode 640) and makes its admin socket through group 10001.

- The **server** serves the client API, which accepts only `Authorization: Bearer <client token>`. Chat replies stream as Server-Sent Events (`meta`, `provider`, `delta`, `done`, `error`) and can be resumed after a dropped connection. Client tokens are random, shown once, and stored as SHA-256 hashes. See [docs/wire-contract.md](wire-contract.md).
- The **admin** listens on a unix socket only. Caddy proxies the site (`<hostname>/admin`) and its API (`<hostname>/api/admin/v1`) to it, and the admin owns `admin.db`. Each section of the site is its own page with its own address (`/admin/providers/`, `/admin/services/`, `/admin/clients/`, `/admin/audit/`, `/admin/backup/`, `/admin/network/`, `/admin/security/`, `/admin/settings/`; `/admin` opens the first), so a page can be bookmarked and Back works. Every change is made in `admin.db` and pushed to the server as one snapshot; if the server is down the change is refused ("The server is offline. Nothing was changed.") rather than half applied. With the server stopped the admin still loads and shows an offline banner.
- The **server API** requires a bearer token. `bootstrap` makes it ([Tokens between services](#tokens-between-services)): the admin reads it from its own volume, `admin-token`, and the server holds only its SHA-256 (`accept/admin-server.sha256` in `server-token`, read per request, compared in constant time, and the API refuses everything when the file is missing).
- **Provider API keys** are kept by the admin, encrypted at rest (ChaCha20-Poly1305) with the admin's own key (`admin-secrets`), and pushed to `model-gateway`, which holds them in memory only ([model-gateway](#model-gateway)). The server holds none. No endpoint returns one. The Cloudflare token is encrypted in `admin.db` with the admin's own key (`admin-secrets`), separate from the server's, and written to a file only Caddy reads.

### First run

From the first start Caddy serves HTTPS on port 443 of every address of the host, with a certificate the admin makes for this install (port 80 redirects). Open **`https://localhost/admin`** on the machine that runs Docker, or `https://<its address>/admin` from another machine, and accept the browser's warning. Until you set a host name, the admin site answers only private addresses (loopback, 10/8, 172.16/12, 192.168/16, Tailscale's 100.64/10 and the IPv6 equivalents) unless the allowlist names others. Caddy checks the connection's real source; a router or proxy that rewrites sources can make public traffic look private. A pairing key made now points at the address you opened the admin with and carries the certificate's fingerprint, so the app trusts exactly this certificate for this server (set `ASKBAR_PUBLIC_URL` in `.env` to choose another address). Once a host name and a real certificate are set (step 5) use `https://<hostname>/admin`.

1. Open the admin (above).
2. Sign in as **`admin`** with the password **`admin`**. Every page but the password change refuses (403 `password_change_required`) until you choose your own: at least 10 characters, not `admin`. Anyone on a private address can use the published login until you do, so do it as soon as the stack is up.
   - **Locked out?** On the host, `docker compose exec admin askbar-admin reset` puts the published login back (you must change it again at the next sign-in) and records the reset on the Services tab; then `docker compose restart admin` to sign out every session. If the admin is not running, use `docker compose run --rm admin reset` instead.
3. **Audit passphrase.** Setup asks for one: at least 20 characters. See *The Audit page* below; write it down.
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

### The Audit page

The stack records a trace of every reply (the auditor keeps it; the admin reads it): when, for whom, through which provider, how long each step took, how many tokens it used, which providers were tried and how it ended. The admin site's **Audit** page (`/admin/audit/`) shows it as an overview, a list of requests, a timeline for each one, routing (which providers answered and how often they failed), and an access history. The address follows the view, so one request can be bookmarked or sent: `/admin/audit/?view=trace&id=<trace id>`.

The **content** of messages (prompts, replies, provider errors) is *sealed*: the server encrypts it with a public key that is made from your audit passphrase (Argon2id to an X25519 key) and never holds the passphrase or the private key. It cannot read what it recorded. To read content, open a request on the Audit page, go to its **Content** view and type the passphrase under **Sealed content**: that **unseals** for a while (you must have signed in within the last ten minutes), and View then shows each item without asking again. The admin derives the keys in memory and never stores them. A banner on every page shows how long it will stay open and has a **Seal** button. It seals itself after **15 minutes without use or an hour in all** (change both in Settings, up to 12 hours each), when you press Seal, when you sign out, or when the admin restarts. A passphrase for an older key, typed while it is open, adds that key. Each unseal, each item viewed, each Seal and each timeout is recorded in the auditor and in the access history (the request, the key and the time, never the content), and an unseal and any change of the timeouts also show as an alert banner for a day, so an unseal you did not do stands out. An unseal is refused if the auditor cannot take its record. An admin API key cannot unseal.

- The passphrase is chosen during setup and **cannot be recovered**. If you lose it, the sealed content recorded with it can never be opened; the rest of the trace (times, tokens, routes, outcomes) stays readable.
- **Settings > Rotate the audit key** makes a new key from a new passphrase. Content recorded earlier stays sealed with the old key and still needs the *old* passphrase.
- Records older than **Keep audit records for (days)** (default 30, at least 7) are deleted once a day. Nothing younger than 7 days can be deleted.
- Until a key exists the server refuses to start replies (`503`), rather than record them unsealed.

The trace lives in the auditor's `audit.db` (`auditor-data` volume). The server and the gateway record their parts of it, and the admin reads it from the auditor with its own read token and applies its prices, so a corrected price changes every figure. If the admin cannot reach the auditor, the Audit page says so and shows nothing until it can.

### CLIProxyAPI

Lets a Claude or ChatGPT subscription act as an OpenAI-compatible provider. It starts with the stack and idles until you connect an account. Everything is done in the admin site, **Services** tab:

1. **Connect Claude** (or **Connect ChatGPT**). The admin asks CLIProxyAPI for a login address.
2. Open the address and approve. The browser then lands on an address starting `http://localhost` that will not load if the admin is on another machine. Copy that whole address from the address bar and paste it into the box. **Finish**.
3. A provider named **CPA** appears on the Providers page, already holding the key the admin made for CLIProxyAPI. Choose its model from the dropdown. Its name, address and key are fixed.

The admin makes CLIProxyAPI's config and API key itself (nobody copies a key anywhere). CLIProxyAPI joins `cpa-gateway` (`model-gateway` reaches it there), `cpa-admin` (the admin runs logins there) and `edge`; its API is never published. See [deploy/cpa/README.md](../deploy/cpa/README.md).

### Configuration reference

**`.env`** (read by Compose; nothing secret):

| Variable | Default | Meaning |
|---|---|---|
| `ASKBAR_PUBLIC_URL` | empty | address put in pairing keys until HTTPS is configured; with HTTPS on it is `https://<hostname>` |
| `DATA_DIR` | `./data` | where the server's `control.db` and `spaces/` live on the host; must be writable by **uid 10002** |

**Upgrading from 0.5 to 0.6 (spaces): wipe the server's data and pair again.** Conversations now live in one file per space ([ADR 0009](adr/0009-spaces-and-storage.md)); before 1.0 nothing is migrated ([ADR 0018](adr/0018-releases-and-migrations.md)), so the server's conversations, paired apps and stored provider keys go. The admin's settings, users and providers stay, and the admin pushes them again; it keeps the key only for its own CLIProxyAPI provider, and sends that one again by itself.

1. `docker compose --profile dev down` (not `-v`: no volume is removed or renamed).
2. Empty the server's data directory: `rm -rf ./data/*` (or your `DATA_DIR`). The server no longer reads `askbar.db`; if one is left it says so in its log and leaves it alone.
3. `docker compose --profile dev up -d --build`.
4. In the admin site, **Providers** shows "key missing on the server" for each provider whose key went with the data: edit it and enter the key again.
5. Update the Askbar app to 0.6.0 (a 0.5 app is told it is too old), create a pairing key for each user in the admin site, and pair each app again.

**Upgrading from 0.4 to 0.5** (stop the stack, pull, rebuild, update the app):

1. Back up first: `data/`, and the `askbar-secrets`, `admin-data`, `admin-secrets` and `audit-data` volumes (see *Backup and recovery*).
2. `docker compose down` (not `-v`), `git pull`, `docker compose up -d --build`. The databases migrate themselves on the first start (the `devices` table becomes `clients`; paired apps and their tokens are kept). <!-- names-ok: the old names are the point of this note -->
3. What moved: the admin is at `https://<hostname>/admin` (the `admin.<hostname>` name and its DNS record are no longer used; before HTTPS was set up 0.5 used `http://127.0.0.1:8081/admin/` on the Docker host; that page is gone, see *First run*). The old token volume `askbar-mgmt` and network `mgmt` are replaced by `server-token` and `server-api`; the admin makes a new server token and writes its hash there on first start, so the old volume can be removed. Data volumes keep their names. CLIProxyAPI now starts with the stack (it idles until you log in). <!-- names-ok: the old names are the point of this note --> If you ran an earlier build of this branch, remove its leftover network first: `docker network rm askbar_setup` (the first-run page now uses `askbar_admin-setup`).
4. Update the Askbar app to 0.5.0. Paired apps keep their tokens, so nothing needs pairing again; an older app says it is too old for the server.
5. If you set any of these in your own compose file, rename them: `ASKBAR_API_ADDR` to `ASKBAR_CLIENT_API_ADDR`, `ASKBAR_MGMT_ADDR` to `ASKBAR_SERVER_API_ADDR`, `ASKBAR_SERVER_MGMT_URL` to `ASKBAR_SERVER_API_URL`, `ASKBAR_MGMT_DIR` to `ASKBAR_SERVER_TOKEN_DIR`, `ASKBAR_MGMT_TOKEN_HASH_FILE` to `ASKBAR_SERVER_TOKEN_HASH_FILE`. <!-- names-ok: the old names are the point of this note -->
6. If anything of yours calls the HTTP APIs: the apps' API is now `/api/client/v1` (`/pair` is `/pairings`, `/whoami` is `/me`), the admin's is `/api/admin/v1`, and every error is a [problem](wire-contract.md#errors) with a stable `error` code.

**Upgrading from 0.2:** the server image now runs as uid 10002 (it was 10001; 10001 is the admin). `chown -R 10002 $DATA_DIR` before starting. The old single-server layout (admin site on :8081, `ASKBAR_UID`/`ASKBAR_GID`, `ASKBAR_API_PORT`, the `tls` profile and Caddy on 8443/9443) is gone, and the first start migrates the database (providers, settings and pairing codes now live in the admin; re-enter them there).

**Set in the admin site** instead of in files: admin password, hostname, Cloudflare token, allowlist, the Let's Encrypt staging switch, the system prompt, display defaults and per-user defaults.

**Compose profiles:** the services (server, admin, Caddy, CLIProxyAPI) are always on. `dev` adds a mock LLM (`tests/e2e/mock_llm.py`).

### Tokens between services

Each link between two services has its own token ([ADR 0004](adr/0004-bootstrap-and-per-link-tokens.md)). A one-shot container, `bootstrap` ([ADR 0049](adr/0049-bootstrap.md)), makes them: it has no network, runs as root with one capability (`CHOWN`), mounts only the credential volumes, and exits. `server` and `admin` start after it has finished. Today there is one link, `admin-server`.

- **Where they are.** One volume per service, `<service>-token`: `server-token` (the server's, as before), `admin-token`, `auditor-token` and `model-gateway-token`. Inside, `call/<link>.token` is a token the service sends and `accept/<link>.sha256` is the hash of a token it checks. A token is mode 400 and its caller's; a hash is mode 444 and its receiver's. Each service mounts only its own volume, read-only.
- **Install.** Nothing to do: `docker compose up -d` runs `bootstrap` first. A second run changes nothing.
- **Upgrade from a build before `bootstrap`.** `docker compose up -d --build`. `bootstrap` makes a new `admin-server` token; the admin reads it and deletes its old `server.token` on start, and the server reads the new hash. Nothing for you to do, and no one is signed out or re-paired. The old `token.sha256` in `server-token` is no longer read; the server refuses to start without the new file and says to run `docker compose up -d`.
- **Rotate** from the Docker host, then restart what it names from the admin site's **Services** page:

  ```sh
  docker compose run --rm bootstrap list                   # the link names
  docker compose run --rm bootstrap rotate admin-server    # one link
  docker compose run --rm bootstrap rotate --service admin # every link of one service, after a suspected compromise
  docker compose run --rm bootstrap rotate                 # every link
  ```

  The old token stops working when the receiver is restarted. Until then, and until the caller is restarted, calls between the two fail.
- **A rejected token.** If the server stops accepting a token that worked (its hash file was replaced, say, or only one side was rotated), the admin site shows a banner with the commands and stops showing "server offline": the server is there, it just refuses this admin. Run `docker compose run --rm bootstrap rotate admin-server`, then `docker compose restart server admin`. The admin cannot restart the server itself, because the server refuses its calls. It logs one line naming the link, never the token, and the banner goes away when the server accepts the token again. The server logs one line when five invalid tokens arrive on its server API within ten minutes, naming the link (`admin-server`) and the count, then stays quiet until the count has dropped below five again ([ADR 0024](adr/0024-rejected-token-alert.md)); the alerter sends these as alerts once it exists (phase 4).
- **Backup.** The credential volumes need none: if they are lost, `bootstrap` makes them again and you restart the services.

### The auditor

The audit store has its own container, `askbar-auditor` ([ADR 0051](adr/0051-auditor.md), [ADR 0010](adr/0010-auditor.md)). It holds `audit.db` on its own volume, `auditor-data`, runs as uid 10003 with no capabilities, and has no route out: it joins only `audit-*` networks, each shared with one caller. Today there are two: `audit-admin`, shared with the admin, and `audit-model-gateway`, shared with `model-gateway`. Its API is `/api/audit/v1` ([docs/openapi/audit.v1.yaml](openapi/audit.v1.yaml)) and it is never public.

**The server and `model-gateway` send it their records** (the gateway opens each reply's turn and try spans and waits for the acknowledgment before it calls a provider; the server adds the gates it passed, its sealed request and reply, and how the reply ended). The server's own `audit.db` is gone: `audit-data` stays on disk untouched and the auditor starts with its own empty `auditor-data`, so records from before the upgrade stay only in the old volume. Copy it (`docker run --rm -v askbar_audit-data:/a -v "$PWD":/b alpine cp -a /a/. /b/audit-backup/`) or delete it once you no longer want them. The admin's Audit pages read the auditor through the server until a later step moves them to the admin.

- **Tokens.** Each link has its own, made by `bootstrap` ([Tokens between services](#tokens-between-services)). The auditor holds only their hashes, in `auditor-token`. Today `admin-auditor-append` (admin action records), `admin-auditor-read` (reads and retention) and `model-gateway-auditor` (the gateway's spans, finish records and sealed payloads) exist; the server's comes when it calls the gateway. A link may call only its own routes, and the caller of a record is the one its token belongs to. No token can change or delete a record.
- **The hash chain.** Every record the auditor takes (a span's start and finish, a sealed payload, an admin action, a prune) is chained: its entry holds a digest of the record and the hash of the entry before it, so changing or removing a record or an entry breaks the chain from there on. The entries are never deleted, not even by retention (they hold digests, not content). **The admin keeps the newest chain head** it has checked, every five minutes compares it with the auditor's, and about once an hour asks the auditor to check every record against its entry. A red banner on every admin page says what it found: **rollback** (the chain is shorter than the admin saw: an older copy of `audit.db` may have been put back, or records removed), **rewritten** (the entry the admin stored is gone or has another hash) or **edited** (records no longer match their entries). The sentence gives numbers and the kind of record, never its content, and it is also logged (`ALERT audit chain`). The banner goes away by itself when the auditor's chain is the one the admin stored again. The chain shows that records were changed; it does not show that a service told the truth. **Restoring `audit.db` from a backup on purpose** shows as a rollback, and the banner stays until there is a way to accept the restored chain (it comes with recording the admin's own actions, plan step 2.7). Records from before the chain existed are chained when the auditor first starts with it, so the chain proves nothing about them before that day.
- **What the admin does is recorded, and refused without a record.** Every change an administrator makes in the admin site or through an admin API key is appended to the auditor **before** it is made: settings (the retention and the kill switch are named in the record, the prompt is not), providers and their tests, users, clients, pairing keys, API keys, the audit key, the admin password, restarts, the network settings, Cloudflare and CLIProxyAPI accounts, and the backup download. The configuration pushes to the server and to the gateway are recorded as actions of their own. If the auditor cannot take the record, the change is refused with a sentence ("The auditor cannot be reached, and every change is recorded first, so nothing was changed") and nothing is saved; reading still works. How each change ended (`done`, `refused` or `failed`) is appended afterwards. A record holds the route, whether a session or a key asked, the ids in the path and a few facts, never what was typed, a secret or message content. `askbar-admin reset` is recorded too, and does not reset the password when the auditor cannot take the record: bring the auditor back first. **The server and the gateway record the configuration version they apply** with their own tokens (`config` / `applied`), so the admin's claim of a push and the service's record sit side by side; a mismatch alert comes later. Sealed-content sessions are recorded too (see the Audit page above). Not recorded yet: signing in and out, and restores (there is no restore route; it is a manual procedure).
- **Caddy's access lines.** Every request Caddy handles is also one line to the auditor, on a network of its own, `audit-caddy`, shared by Caddy and the auditor alone. Caddy's log writer cannot carry a token, so the auditor takes the lines on an ingest listener (`auditor-ingest-host:8086`) that reads and never answers: from `audit-caddy`, Caddy can add lines and nothing else, and the token API is not bound on that network at all (the API listens on one address for each of its callers' networks, not on every interface). A line keeps the time, method, host, path **without its query string**, status, size, duration, the client address, the protocol and a cut user agent: no header and no query string is kept, because either can carry a secret. The lines are chained and pruned with the other records, and the admin read token reads them (`GET /access-logs`); there is no page for them yet. They are best effort, unlike model calls: Caddy starts and serves while the auditor is away (`soft_start`), a line that cannot be sent is lost, and when the auditor is busy it drops lines (never more than half its queue) and counts them in its status (`access_lines_dropped`, `access_lines_refused`). **Upgrading:** `docker compose up -d --build` adds the network and the listener, and the auditor's first start applies migration 5, which makes the chain table again with the new kind and keeps every entry.
- **Retention.** Records are kept 30 days by default and never less than 7. The admin read token sets it; the auditor prunes once a day, and each prune leaves a record of its own.
- **When it refuses.** Past its backlog limit (1,000 queued records, or the oldest waiting over 5 seconds) it refuses every append, and below 256 MiB free disk it refuses a new span, with `503 audit_backlog`. A model call that cannot be recorded does not start. That is deliberate: model work stops rather than go unrecorded.
- **Acknowledgment time.** `cargo test -p askbar-auditor --test latency -- --nocapture` measures it (about 1.5 ms at the 95th percentile on a development machine, against a 20 ms target). It is not a gate until the ship checks.
- **Upgrade.** `docker compose up -d --build`. `bootstrap` makes the new tokens, the auditor starts empty, and nothing else changes.

### model-gateway

Provider calls and provider keys have their own container, `askbar-model-gateway` ([ADR 0054](adr/0054-model-gateway.md), [ADR 0014](adr/0014-model-gateway-and-providers.md)). It runs as uid 10004 with no capabilities and no data volume: **the keys live in its memory only.** Its API is `/api/model-gateway/v1` ([docs/openapi/model-gateway.v1.yaml](openapi/model-gateway.v1.yaml)) and it is never public. It has two listeners, one on each network it shares with a caller, each on the address its alias gives (`gateway-server-host`, `gateway-admin-host`, port 8084) and never a wildcard, because it also joins `outbound`, the one network besides `edge` with a route out. Direct outbound access is how v1 reaches providers ([ADR 0006](adr/0006-v1-outbound-access-and-alerts.md)).

**The server asks it for every reply**, on `gateway-server`, with its own token, and has no key and no route to a provider. If the gateway has no configuration yet (it just started) or the auditor cannot take the record, the reply fails with a sentence and nothing is sent to a provider; ask again in a few seconds.

- **Keys.** You enter a provider's key in the admin site as before. The admin now keeps it too, encrypted with its own key (`admin.key`, in the `admin-secrets` volume), and pushes it to the gateway in `PUT /config`; no page or endpoint shows it again. The admin checks the gateway every 12 seconds and pushes whenever it has no configuration or another one, so **a restarted gateway, or a rebooted host, gets its keys back with no command**, once the admin is running. The **Services** page shows the gateway and can restart it.
- **Upgrading: enter each provider's key again.** Before this version the server held the keys and the admin could not read them, so after the upgrade no provider has a key in the gateway. The Providers page marks each one **key missing**; open it, choose Edit and enter its key again. Until you do, replies on that provider fail with a sentence saying the provider did not accept its key. The CLIProxyAPI provider needs nothing: the admin's own file holds its key. The old keys stay encrypted in the server's database (`data/control.db`, with `master.key` in the `askbar-secrets` volume) and nothing reads them; delete them with those volumes if you wish.
- **What it records.** Before it calls a provider it appends the turn span to the auditor and waits for the acknowledgment (one second at most); with none it answers `503 audit_backlog` and calls nothing. Each try at a provider is a span of its own, acknowledged the same way. A provider's error text goes to the auditor sealed to your audit key, never to a log and never to the server. A turn the server does not finish is ended as `interrupted` a minute after the generation time limit.
- **Tokens.** `server-model-gateway` (the server calls it) and `admin-model-gateway` (the admin pushes and restarts) are made by `bootstrap`, and `model-gateway-auditor` (it appends to the auditor). Each listener takes only its own link's token. The admin will not start without its token: run `docker compose up -d`, which runs `bootstrap` first.
- **Upgrade.** `docker compose up -d --build`. `bootstrap` makes the three new tokens, the admin keeps working with the keys it has, and the gateway starts with none until the admin pushes (seconds). `docker compose --profile dev` also puts the mock LLM on `outbound`.

### Starting and stopping services

The admin site's **Services** page shows whether the server, `model-gateway`, the admin, Caddy and CLIProxyAPI answer, and can restart each (it asks first and says what that will interrupt). No service has the Docker socket ([ADR 0005](adr/0005-no-controller-and-self-restart.md)): the server and the admin restart themselves through their own endpoints and Docker's restart policy starts them again (the server is back within a minute; you sign in to the admin again), Caddy has its configuration pushed again, and CLIProxyAPI is restarted by a small supervisor in its image (`askbar-cpa`, [ADR 0037](adr/0037-askbar-cpa-image-and-supervisor.md)), which also restarts it when its config changes or it crashes. A service refuses a restart in its first 30 seconds, and the server and admin also restart themselves if their own health check fails three times in a row. Starting or stopping a service is done from the host (`docker compose stop server`). Only a signed-in person can restart, not an admin API key; each attempt is recorded, and six a minute is the limit. See [SECURITY.md](../SECURITY.md) for what this trusts.

**Upgrading from an install with the controller:** `docker compose up -d --remove-orphans` removes the old `askbar-controller` container; the `control` network goes with it. The `control-token` volume stays declared and unused until 1.0.

**Upgrading an install that used `cpa-init` / `cpa-login`:** CLIProxyAPI's config volume was root-owned; the admin (uid 10001) must be able to read and write it once: `docker compose run --rm --no-deps --user root --entrypoint chown admin -R 10001:10001 /cpa-config`, then `docker compose up -d --build`. The admin keeps your existing API key (so the provider and your logins carry on), replaces CLIProxyAPI's control secret and restarts it. If the admin cannot read the config it says so in its log and leaves CLIProxyAPI alone.

**Server environment** (already set in `compose.yaml`; useful without Docker): `ASKBAR_CLIENT_API_ADDR`, `ASKBAR_SERVER_API_ADDR` (`ip:port`, or `host:port` resolved once at start-up to exactly one address; never `0.0.0.0`), `ASKBAR_SERVER_TOKEN_HASH_FILE`, `ASKBAR_DATA_DIR`, `ASKBAR_SERVER_TOKEN_DIR` (its tokens for the gateway and the auditor), `ASKBAR_GATEWAY_URL` and `ASKBAR_AUDIT_API_URL`. **Model-gateway:** `ASKBAR_GATEWAY_SERVER_ADDR`, `ASKBAR_GATEWAY_ADMIN_ADDR`, `ASKBAR_GATEWAY_TOKEN_DIR`, `ASKBAR_AUDIT_API_URL`. **Admin:** `ASKBAR_ADMIN_SOCKET`, `ASKBAR_ADMIN_DATA_DIR`, `ASKBAR_ADMIN_SECRETS_DIR`, `ASKBAR_ADMIN_TOKEN_DIR`, `ASKBAR_SERVER_API_URL`, `ASKBAR_CADDY_SOCKET`, `ASKBAR_CADDY_SECRETS_DIR`, `ASKBAR_PUBLIC_URL`. **Auditor:** `ASKBAR_AUDIT_API_ADDR` (default `0.0.0.0:8083`), `ASKBAR_AUDIT_DIR`, `ASKBAR_AUDITOR_TOKEN_DIR`.

### Backup and recovery

Four things to back up, **separately**:

- **`data/`** (the server's `control.db`: users, paired clients and spaces; and `spaces/`, one file per space with its conversations) and the **`admin-data`** volume (the admin's database).
- **The `admin-secrets` volume** (`admin.key`). Keep it somewhere other than your database backups: anyone holding both it and `admin-data` can read the stored provider keys. For example: `docker run --rm -v askbar_admin-secrets:/s -v "$PWD":/b alpine cp /s/admin.key /b/admin.key`, then store the copy in a password manager. (`askbar-secrets`, which held the server's old master key, is no longer used.)
- Admin site > **Backup** downloads one `.tar` with `server/control.db`, `server/spaces/<space id>.db` for every space, and `admin.db`. It is consistent (`VACUUM INTO`) and leaves out provider API keys, pair codes, the Cloudflare token and the admin password hash, so after restoring it you enter those again.

- **The `auditor-data` volume** (`audit.db`, the trace of replies) is **not** in the admin site's Backup `.tar`. Copy the whole volume if you want it (stop the auditor first, or copy `audit.db`, `audit.db-wal` and `audit.db-shm` together): `docker run --rm -v askbar_auditor-data:/a -v "$PWD":/b alpine cp -a /a/. /b/audit-backup/`. A copy holds sealed content only, which is useless without the audit passphrase. (`audit-data` is the server's old store; see [The auditor](#the-auditor).)

A restored `data/` needs no key volume: the server holds no provider key. After a restore, enter the provider keys again on the Providers page if the admin's database was restored without `admin-secrets`.

## Layout

```
crates/askbar-proto/   request/response and snapshot types shared by the server, admin and client
crates/askbar-service/ what every service shares: per-link token checks, GET /status, the not-configured gate
crates/askbar-audit/   the audit store (spans and sealed payloads in audit.db); the server links it today
services/bootstrap/    makes the per-link tokens and exits; rotates them from the host
services/auditor/      the audit store as its own container: /api/audit/v1, tokens per link, retention, limits
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

The target layout is in [ADR 0033](adr/0033-repository-layout.md).

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

Every pull request runs `.github/workflows/ci.yml` on Linux: rustfmt and clippy in both workspaces, `npm test` (as two parallel jobs, `test:services` and `test:desktop`), the smoke test, the isolation checks against a stack it builds, `cargo deny` and the npm license and advisory checks, and a gitleaks scan of the history. The same advisory checks run weekly. `build.yml` builds and tests macOS and Windows, by hand.

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

Askbar is licensed under the [GNU Affero General Public License v3.0 or later](../LICENSE) (AGPL-3.0-or-later).

**Source code:** <https://github.com/Thewuuryar/askbar>. If you run a modified server for other people, section 13 of the AGPL requires offering them the corresponding source; the admin page links here for that reason. The Caddy and CLIProxyAPI containers are separate programs with their own licenses.
