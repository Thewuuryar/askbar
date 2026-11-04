# 0002. One name per role, one host name

Status: Proposed
Date: 2026-11-03

## Context

Earlier builds used several words for the same thing and gave the admin its own subdomain. More components are likely, and family accounts are planned. The admin must be usable from any computer or phone that can reach its URL, and the whole server side should have one address.

## Decision

1. **One name per role.** Every role has exactly one name, used in API paths, code, environment variables, UI labels and documents. The names and their meanings are in the glossary in [architecture.md](../architecture.md); `scripts/check-names.sh` enforces them and keeps retired words out.
2. **A part becomes a named component only when it gets its own container.** A new component gets a name, and an ADR, before it exists. Its name must not be confusable with an existing one.
3. **One public host name for the server side.** The admin site is served at `/admin` and the client API at `/api/client`. Caddy answers every other path with a 404. The admin is reachable from any computer or phone that can reach the host name; its protection is its sign-in ([0008](0008-admin-sign-in-and-exposure.md)), not its network position.
4. **Admin and server are separate programs that talk through the server API.** Sharing a host name does not mean sharing a process, container or storage ([0003](0003-admin-and-server-isolation.md)).
5. **Paths name the audience first and the version after it**, so each API versions independently (`/api/client/v1/...`, `/admin/api/v1/...`).

## Alternatives considered

- **Admin on its own subdomain.** Gives the admin its own browser origin. Rejected by the owner on 2026-11-03: one host name and one URL are preferred, and the isolation that matters is the container boundary.
- **Admin reachable only from the home network or a VPN.** Rejected by the owner on 2026-11-03: the admin should work from any computer or phone.

## Consequences

- One DNS record, one certificate, one URL to remember.
- The client API and the admin share a browser origin. Their separation rests on the admin cookie (`Path=/admin`, `HttpOnly`, `SameSite=Strict`, `__Secure-` prefix), a CSRF header, a strict CSP, and Caddy's 404 for everything else. Two rules keep that sound:
  - Nothing on this host may serve user-controlled content as HTML or script. The client API returns JSON only, with `X-Content-Type-Options: nosniff`.
  - The admin refuses cross-site requests (`Sec-Fetch-Site`) as well as requiring the CSRF header.
- The admin is reachable from the internet by design, so sign-in is the only perimeter ([0008](0008-admin-sign-in-and-exposure.md)).
- Passkeys are bound to the host name. Changing the host name later invalidates them.
- A new component with its own audience needs a name, a path prefix, an OpenAPI file and an ADR.
