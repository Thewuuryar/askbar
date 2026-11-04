# 0003. The admin and the server are isolated

Status: Accepted
Date: 2026-11-03 (backfilled)

## Context

Goal from the goals document: "Keep admin separate, so running agents can never change settings", and "nothing on the chat side can write back to admin". The server talks to model providers and handles untrusted text; the admin holds the settings and the password.

## Decision

- The admin and the server run in different containers, with different uids and different volumes. The server cannot read `admin.db`, `admin.key` or the admin's socket; the admin cannot read `master.key` or `audit.db`.
- Configuration moves one way: the admin pushes one snapshot to the server over the server API. The server API listens on one fixed address on an `internal` network that only the admin and the server join, and requires a bearer token. The admin holds the token; the server holds only its SHA-256, read-only.
- The server never calls the admin.

Reasons for each boundary are in [architecture.md](../architecture.md), "Why each boundary exists"; what each part can read is the table in [SECURITY.md](../../SECURITY.md).

## Alternatives considered

Not recorded.

## Consequences

An admin change is refused when the server is down, rather than half applied. A compromised server cannot change settings or mint a token. Enforced by `scripts/check-isolation.sh` (needs Docker; not run in CI) and by the server API tests. Anything that gives the server side a path to the admin breaks this ADR.
