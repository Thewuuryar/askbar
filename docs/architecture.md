# Architecture

This document is the target the code follows. If code and this file disagree, one of them is a bug; fix it in the same commit.

## Glossary

Every API, module, environment variable, UI label and document uses these names ([ADR 0002](adr/0002-one-name-per-role-one-host.md)). This glossary is not an ADR: names and meanings can change here, in the same commit as the code that follows them. Wire-visible names (paths, headers, JSON fields, error codes) stay as they are within `v1`.

### Components

| Name | Means | API | Served by | Called by |
| --- | --- | --- | --- | --- |
| **client** | one paired app instance (the desktop app today) and its token; belongs to exactly one user | `/api/client/v1` | `askbar-server` | the app, or anyone's client |
| **server** | the Askbar server, and its own control and inspection interface: configuration snapshot, provider keys, usage, audit, backup | `/api/server/v1` | `askbar-server`, internal listener only | `askbar-admin` |
| **admin** | the admin site and its control interface | `/admin/api/v1` | `askbar-admin` | the admin site (session cookie) or a script (admin API key) |
| **controller** | the one container with the Docker socket; starts, stops and restarts `server`, `caddy` and `cpa` | `/api/control/v1` | `askbar-controller`, internal network only | `askbar-admin` |
| **executor** (reserved) | the container that will run tools, with no route to the server, admin or controller; nothing uses it yet ([ADR 0007](adr/0007-conversation-only-and-tool-gating.md)) | none | none | none |

Third-party components keep their product name (`caddy`, `cpa`). First-party components get a role name. The model gateway is a module inside `server` and has no component name until it gets its own container.

### Other terms

| Name | Means |
| --- | --- |
| **user** | a person who owns conversations and memory (family accounts, later) |
| **provider** | an upstream source of model output (`cpa`, a paid API) |
| **account** | a login held inside a provider, such as a Claude or ChatGPT login in `cpa`; never called a provider |
| **token** / **key** | token: a bearer secret Askbar issues. Key: a cryptographic key, or a third party's API key |
| **settings** / **snapshot** | settings: what the owner edits in the admin. Snapshot: the object pushed to the server |
| **audit** / **trace** / **span** | audit: the feature. Trace: the record of one reply. Span: one step in it |

Retired words, refused by `scripts/check-names.sh`: "management" and "mgmt" (now **server**), "device" (now **client**).

## Picture

```
 desktop app  --HTTPS, client token-->  Caddy :443 --/api/client/v1/*--> askbar-server :8080 --> providers
 browser      --HTTPS, session cookie-> Caddy :443 --/admin, /admin/*--unix socket--> askbar-admin
 script       --HTTPS, admin API key--> Caddy :443 --/admin/api/v1/*---unix socket--> askbar-admin
                                                                                          |
                                                    server API (server token, internal network only)
                                                                                          v
                                                                                   askbar-server :8082

 admin --control network, controller token--> askbar-controller --docker.sock--> Docker (start/stop/restart server, caddy, cpa)
 admin --cpa-admin network, control secret--> cpa :8317   (account logins: connect Claude / ChatGPT)
```

Caddy is the only service with published ports. `askbar-server` serves two listeners: the client API (reachable from Caddy) and the server API (reachable from the admin only).

## Routes

One public host. The path rule is `/<audience>/<...>/v<N>/<resource>`: the audience comes first and the version after it, so each API versions independently.

| Public path | Goes to | Who can reach it | What protects it |
| --- | --- | --- | --- |
| `/admin`, `/admin/*` (site files and `/admin/api/v1/*`) | admin unix socket | anyone Caddy lets through | session cookie plus `X-Askbar-Admin: 1` on changes, or an admin API key as `Authorization: Bearer`; optional `remote_ip` allowlist; login lockout |
| `/api/client/v1/*` | `server:8080` | anyone | client bearer token plus the client-version gate (`health` and `pairings` need no token) |
| `/api/server/*` | answered 404 by Caddy | nobody from outside | not routed; the real listener is on the internal `server-api` network |
| everything else | answered 404 by Caddy | n/a | n/a |

Within `v1`, changes are additive only (new fields, endpoints, error codes). Anything else gets `v2` beside `v1` for one release.

## Errors

Every failure is `application/problem+json` (RFC 9457) with `type` (`urn:askbar:problem:<code>`), `title`, `status`, `detail` (a sentence for a person), `error` (the stable code clients branch on) and extension members such as `period`. Status codes and `Retry-After` are part of the contract. Clients map by `error`, never by prose. The registry of codes lives in `askbar-proto/src/problem.rs`.

## Versions

The version in the URL is the API version. `X-Askbar-Client-Version` and the 426 `client_too_old` response remain for one case only: the client is too old for this server within `v1`. The header applies to the client API only.

## Volumes and networks

Data volumes must never be renamed: Compose would silently create an empty one (for `askbar-secrets` that orphans the master key).

| Name | Holds | Mounted by |
| --- | --- | --- |
| `./data` (bind, `DATA_DIR`) | `askbar.db`: conversations, clients, encrypted provider keys | server (uid 10002) |
| `askbar-secrets` | `master.key` | server only |
| `audit-data` | `audit.db`, the trace of replies (content sealed) | server only |
| `server-token` | `token.sha256`, the server token's hash (regenerable) | admin read-write, server read-only |
| `admin-data` | `admin.db` | admin only |
| `admin-secrets` | `admin.key` | admin only |
| `admin-run` | the admin's unix socket | admin, Caddy |
| `caddy-run` | Caddy's admin socket | Caddy, admin |
| `caddy-secrets` | the Cloudflare token file | admin read-write, Caddy read-only |
| `caddy-data`, `caddy-config` | certificates and Caddy state | Caddy |
| `control-token` | `token.sha256`, the controller token's hash (regenerable) | admin read-write, controller read-only |
| `cpa-config` | CLIProxyAPI's `config.yaml`: the admin makes it (API key, control secret) | admin read-write, cpa |
| `cpa-auth` | the Claude / ChatGPT logins | cpa only |
| `/var/run/docker.sock` (host) | Docker | controller only |

| Network | Members | Why |
| --- | --- | --- |
| `edge` | Caddy, server, cpa (and the dev mock) | the only network with a route out; only Caddy publishes ports |
| `control` | admin, controller | `internal`; the controller has no other network and no ports |
| `cpa-admin` | admin, cpa | `internal`; how the admin runs CLIProxyAPI logins without ever joining `edge` |
| `server-api` | admin, server | `internal`, so it has no route out; the server API listens on one fixed address, never a wildcard |
| `admin-setup` | Caddy alone | the first-run page for the admin on `127.0.0.1:8081` (plain HTTP). Nothing else joins, so no container can open it. The name must sort before `edge`: Docker publishes a container's ports on its address on the first network in name order, and Caddy listens for this page on its `admin-setup` address only |

## Why each boundary exists

- **Server API on its own internal listener.** It can replace the whole configuration and read the audit trace. It is never routed publicly; Caddy answers 404 for `/api/server/*` as a second layer.
- **Server token, hashed on the server side.** The admin holds the token, the server holds only its SHA-256 on a volume it mounts read-only, so a compromised server cannot mint a token.
- **Admin on a unix socket, one network.** No TCP port exists to attack; only Caddy shares the socket's volume.
- **Admin and server in different containers with different uids.** The server cannot read `admin.db`, the admin cannot read `master.key` or `audit.db`.
- **Audit content sealed.** The server holds only a public key derived from the audit passphrase, so it cannot read what it recorded.
- **One origin for client and admin.** The client API serves JSON only and the admin cookie is `Path=/admin`, `HttpOnly`, `SameSite=Strict` with a CSRF header and a strict CSP, so sharing a host name does not let one reach the other's credentials. The remaining protection is Caddy's 404 for every path outside the three prefixes.
- **CLIProxyAPI on `edge` and `cpa-admin`.** The server reaches it on `edge`; the admin reaches its account-login API on `cpa-admin`, which has no route out. Nothing publishes it and Caddy has no route to it.
- **The Docker socket only in the controller.** Service control needs it, and it is root on the host, so it lives in a container with nothing else: no ports, no route out, one network shared with the admin, a token, and an allow list of three services (see SECURITY.md). The admin cannot start anything new.
- **The admin makes CLIProxyAPI's config.** The CPA API key and control secret are generated by the admin (kept in `admin-secrets`) so no one copies a key; the provider for CLIProxyAPI is created by the admin and its name, address and key are fixed.

## Decisions

- **Chat stays `POST /api/client/v1/chat`.** It creates the conversation if needed, saves the question, starts a generation and streams it in one call, with `client_request_id` idempotency the outbox relies on. It is the one RPC-style endpoint.
- **Old clients.** `/api/v1/*` answers one 426 `client_too_old`; nothing else of the old paths exists.
- **Client tokens and pairing strings stay valid** across the upgrade: nothing stored changes.
- **The admin API is public and documented**, with its own OpenAPI file; scripts use an admin API key.

## Considered and not changed

`askbar-proto` (shared types), cookie and header names, the `askbar-pair:` prefix, container and image names, `src-tauri/` and `ui/` directory names (Tauri convention), data volume names.
