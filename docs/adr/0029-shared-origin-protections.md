# 0029. Shared origin protections

Status: Accepted
Date: 2026-11-04
Source: required by v1 decisions

## Context

The admin and client API share one host name, and so one browser origin. Something on that origin must not be able to use the admin's session.

## Decision

1. **Admin cookie:** it uses the `__Secure-` prefix, `Secure`, `HttpOnly`, `SameSite=Strict`, and `Path=/api/admin`.
2. **Changes:** the admin requires a CSRF header on changes, and refuses cross-site requests using `Sec-Fetch-Site`.
3. **Content:** nothing on this host serves user-controlled content as HTML or script.
4. **Client API:** it returns JSON or its documented event stream, with `X-Content-Type-Options: nosniff`.
5. **Admin site:** it keeps a strict CSP, which requires Trusted Types (`require-trusted-types-for 'script'`), as does the desktop popup's ([0044](0044-desktop-client.md)).
6. **Routing:** Caddy answers 404 for anything except `/admin`, `/api/admin`, and `/api/client`.

## Alternatives considered

- **A separate admin host name:** this would give the admin its own origin, but [0003](0003-v1-service-layout.md) keeps one host name.

## Consequences

The admin and client API can stay on one URL, but these rules become part of the security boundary. Browser and API tests must cover each rule, and a new route that serves content needs a review against them.

## Amendments
- 2026-11-11: Trusted Types required in the admin CSP (#70, D17, a decision change); built in plan step 5.2.
