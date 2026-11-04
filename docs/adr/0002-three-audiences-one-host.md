# 0002. Three audiences, one vocabulary, one host name

Status: Accepted (shipped in 0.5.0)
Date: 2026-11-03 (backfilled)

## Context

Askbar has three kinds of caller: the apps, the admin service and its site, and the server's own internal interface. Earlier builds used several words for the same things and an `admin.<hostname>` name with its own DNS record. The goals document ("Architecture Goals and Features") still describes the admin on its own subdomain.

## Decision

- Exactly three words, plus one: **client**, **server**, **admin**, and **controller** ([architecture.md](../architecture.md), "The three words"). `scripts/check-names.sh` enforces them.
- Paths follow `/<audience>/<...>/v<N>/<resource>`, so each API versions on its own.
- One public host name serves the admin at `/admin` and the client API at `/api/client`; every other path is a 404 from Caddy. The admin's cookie is `Path=/admin`, `HttpOnly`, `SameSite=Strict`, with a CSRF header and a strict CSP. An optional address allowlist limits who can reach `/admin`.

## Alternatives considered

A separate admin subdomain (the goals document, and builds before 0.5.0). Rejected by the owner on 2026-11-03: one domain and one certificate, with the admin at `/admin`. The admin still runs in its own isolated container ([0003](0003-admin-and-server-isolation.md)); sharing a host name does not mean sharing a process.

## Consequences

One certificate and one DNS record. The admin and the client API share an origin, so their separation depends on the cookie scope, the CSRF header, the CSP and Caddy's 404 for everything else. A new audience needs a new prefix and its own OpenAPI file, and an ADR.
