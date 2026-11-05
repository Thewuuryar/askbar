# Architecture

This document is the proposed v1 target that the code will follow. The code on `main` doesn't fully match it yet, and each [ADR](adr/README.md) states whether it's Proposed or Accepted.

## Glossary

Every API, module, environment variable, UI label and document uses these names ([ADR 0021](adr/0021-names-and-new-components.md)). This glossary is not an ADR: names and meanings can change here, in the same commit as the code that follows them.

### Components

| Name | Means | API | Served by | Called by |
| --- | --- | --- | --- | --- |
| **client** | one paired app instance and its token; belongs to exactly one user | `/api/client/v1` | `askbar-server` | the app |
| **server** | the Askbar server: chats, spaces, and its control and inspection interface | `/api/server/v1` | `askbar-server`, internal listener only | `askbar-admin` |
| **admin** | the admin site and its control interface | `/api/admin/v1` | `askbar-admin` | the admin site (session cookie) or a script (admin API key) |
| **auditor** | the container that keeps the audit store: services append to it, and the admin reads it | `/api/audit/v1` | `askbar-auditor`, internal only | services (append), `askbar-admin` (read) |
| **model-gateway** | the container that holds provider keys in memory, enforces budgets and the kill switch, and calls providers | `/api/model-gateway/v1` | `askbar-model-gateway`, internal only | `askbar-server` |
| **alerter** | the small container that delivers alerts and checks for new releases | `/api/alerter/v1` | `askbar-alerter`, internal only | `askbar-auditor`, `askbar-admin` |
| **bootstrap** | the one-time container that creates per-link tokens on first start | none | none | none |

Third-party components keep their product name (`caddy`, `cpa`). After v1, these names are added: **egress proxy**, **MCP gateway**, **judge**, **executor**, **`executor-link`** and `searxng` ([ADR 0019](adr/0019-post-v1-order-and-tools.md), [ADR 0020](adr/0020-remote-executor-and-ca-triggers.md)).

### Other terms

| Name | Means |
| --- | --- |
| **user** | a person the admin creates, who owns clients and a personal space |
| **space** | one SQLite file of conversations, history, memory and search: personal or shared |
| **pairing key** | a one-time key that registers one client for a user |
| **provider** | an upstream source of model output (`cpa`, a paid API, a self-hosted model) |
| **account** | a login held inside a provider, such as a login in `cpa`; never called a provider |
| **token** / **key** | token: a bearer secret Askbar issues. Key: a cryptographic key, or a third party's API key |
| **settings** / **snapshot** | settings: what the owner edits in the admin. Snapshot: the object pushed to a service |
| **audit** / **trace** / **span** | audit: the feature. Trace: the record of one reply. Span: one step in it |

Retired words, refused by `scripts/check-names.sh`: "management" and "mgmt" (now **server**), "device" (now **client**).

## Picture

```
 client  --HTTPS, client token---> Caddy --/api/client/v1/*--> server --> model-gateway --> providers, GPU desktop
 browser --HTTPS, session cookie-> Caddy --/admin, /api/admin/v1/*--unix socket--> admin    |
 script  --HTTPS, admin API key--> Caddy --/api/admin/v1/*----unix socket--> admin          +--> cpa --> account providers

 admin --config, restarts, backup exports--> each service
 admin --read token--> auditor <--append tokens-- each service except the admin
 auditor --alert events--> alerter --> ntfy or email, dead-man monitor, release check
 admin --encrypted backups--> /backup volume or SMB/NFS share volume
```

Caddy is the only service with published ports. The server stores no secrets and has no outbound route.

## Routes

One host name. Every API uses `/api/{name}/v{major}` ([ADR 0022](adr/0022-api-path-conventions.md)).

| Public path | Goes to | Who can reach it | What protects it |
| --- | --- | --- | --- |
| `/admin`, `/admin/*` (site files) | admin unix socket | private addresses by default | `remote_ip` allowlist |
| `/api/admin/v1/*` | admin unix socket | private addresses by default | session cookie and CSRF header on changes, or an admin API key; allowlist; login lockout |
| `/api/client/v1/*` | `server` | local network or Tailscale by default | client bearer token plus the client-version gate |
| everything else, including other `/api/*` paths | answered 404 by Caddy | n/a | n/a |

Before 1.0, paths can change to fit the conventions. After 1.0, `/api/client/v1` changes are additive only.

## Errors

Every failure is `application/problem+json` (RFC 9457) with `type` (`urn:askbar:problem:<code>`), `title`, `status`, `detail` (a sentence for a person), `error` (the stable code clients branch on) and extension members such as `period`. Status codes and `Retry-After` are part of the contract. Clients map by `error`, never by prose. The registry of codes lives in `askbar-proto/src/problem.rs`.

## Versions

The version in the URL is the API version. `X-Askbar-Client-Version` and the 426 `client_too_old` response remain for one case only: the client is too old for this server. The header applies to the client API only.

## Links

Every link has its own authenticated path and uses separate networks, so a missing row means a blocked path.

| From | To | Purpose | Authentication |
| --- | --- | --- | --- |
| Client, browser, script | Caddy | Public entry | TLS, then the target API's credential |
| Caddy | `server` | Client API | Client token at the server |
| Caddy | `admin` | Admin site and API, over a unix socket | Admin credential at the admin |
| `admin` | Caddy | Configuration, over Caddy's admin socket | Socket permissions |
| `admin` | `server`, `model-gateway`, `cpa` | Status, configuration, restarts, logins | Per-link token |
| `admin` | Each exporting service | Encrypted backup exports | Per-link token |
| `admin` | `auditor` | Read logs and set retention | Admin read token |
| `admin` | `alerter` | Read release results | Per-link token |
| Each service except the admin | `auditor` | Append records | Service append token |
| `auditor` | `alerter` | Alert events | Per-link token |
| `server` | `model-gateway` | Model calls | Per-link token |
| `model-gateway` | `cpa` | Subscription providers | `cpa` API key |

**Outbound access:** in v1, Caddy, `model-gateway`, `cpa` and `alerter` have direct outbound access. The server and auditor have none. The egress proxy ships first after v1.

## Volumes

Each service has its own data and credential volumes and user ID, with no shared data volumes or host bind mounts ([ADR 0003](adr/0003-v1-service-layout.md)). Data volumes must never be renamed: Compose would silently create an empty one.

| Holds | Mounted by |
| --- | --- |
| Control database and space files | `server` |
| Audit store | `auditor` |
| Admin data and secrets | `admin` |
| Per-link token files and hashes | `bootstrap` once, then the service that owns each file |
| Certificates and Caddy state | `caddy` |
| `cpa` config | `cpa`, written by `admin` |
| `cpa` logins | `cpa` |
| `alerter` state and delivery settings | `alerter` |
| Admin and Caddy unix sockets | `admin` and `caddy` |
| `/backup` or SMB/NFS share volume, defined in Compose | `admin` |

## Why each boundary exists

- **Server API on its own internal listener.** It can replace the server's configuration, so it is never routed publicly.
- **Per-link tokens from bootstrap.** Each receiver holds a hash and maps it to one caller and its allowed routes, so a compromised service can use only its own token.
- **Admin on a unix socket.** No admin TCP port exists to attack through Caddy.
- **Admin and server in different containers with different user IDs and images.** The server can't read the admin's data, and the server never calls the admin. Services answer `503 not_configured` until the admin pushes their configuration ([ADR 0028](adr/0028-service-boundaries-and-config-delivery.md)).
- **Provider keys in `model-gateway`.** The server has no keys or outbound route, so a compromised server can't copy keys or send data directly.
- **Auditor in its own container.** It accepts appends only, with a hash chain, so a compromised server or admin can't rewrite earlier records. A provider call doesn't start until the auditor acknowledges its span ([ADR 0031](adr/0031-audit-span-start-and-backlog.md)).
- **Audit content sealed.** The server holds only a public key derived from the audit passphrase, so it can't read what it recorded ([ADR 0030](adr/0030-sealed-audit-content.md)).
- **One file per space.** A query bug in one space can't reach another space's file ([ADR 0009](adr/0009-spaces-and-storage.md)).
- **One origin for client and admin.** The client API serves JSON or its documented event stream, and the admin cookie uses `Path=/api/admin`, `HttpOnly` and `SameSite=Strict`, with a CSRF header, `Sec-Fetch-Site` checks and a strict CSP ([ADR 0029](adr/0029-shared-origin-protections.md)).
- **No Docker socket.** Docker access could mount any volume, so services restart themselves through their own endpoints ([ADR 0005](adr/0005-no-controller-and-self-restart.md)).
- **Container hardening.** Every service uses a read-only root filesystem, dropped capabilities and limits ([ADR 0032](adr/0032-container-hardening.md)).
- **Encrypted exports.** Each service encrypts its backup export, so the admin bundles files it can't read ([ADR 0013](adr/0013-backups.md)).

## Decisions

- **Chat is `POST /api/client/v1/spaces/{space_id}/chat`.** It creates the conversation if needed, saves the question and starts a generation, with `client_request_id` idempotency. Chats are processed on the server and sent to clients when complete. Live sync happens only on the client that started the chat, or it shows a processing message ([ADR 0008](adr/0008-users-clients-and-client-access.md)).
- **Old clients.** `/api/v1/*` answers one 426 `client_too_old`; nothing else of the old paths exists.
- **The admin API is documented**, with its own OpenAPI file; scripts use an admin API key.

## Considered and not changed

`askbar-proto` (shared types), the `askbar-pair:` prefix, `src-tauri/` and `ui/` directory names (Tauri convention).
