# 0004. Bootstrap and per-link tokens

Status: Accepted
Date: 2026-11-04
Plan decisions: Bootstrap and per-link tokens; CA as optional

## Context

Services must connect automatically, with no manual commands, on one Docker host. Service identity should stop one service from impersonating another without adding a certificate system to operate.

## Decision

- **Bootstrap:** `bootstrap` runs before the other services and has no network. It mounts only credential volumes, generates each per-link token, writes it to the caller's volume, and writes its hash to the receiver's volume. It then exits, and the other services start. It also runs from the host on demand to rotate tokens ([0005](0005-no-controller-and-self-restart.md)).
- **Credentials:** credentials use named volumes instead of Compose file secrets, because those files must exist before startup. Internal service names are fixed in the images or Compose file.
- **Authorization:** each receiver maps a token to one caller and its allowed routes.
- **CA:** v1 has no CA. A CA is optional in v2, and only if a trigger in [0020](0020-remote-executor-and-ca-triggers.md) is met.

## Alternatives considered

- **A private CA with mutual TLS on every link:** rejected for v1. It adds certificate issuance and renewal, plus an outage risk if the admin can't renew certificates.
- **First-contact claims:** rejected. Bootstrap needs no claim window or source-address check.

## Consequences

Setup needs no manual wiring, and the admin's downtime can't expire a service credential. A stolen token works until it's rotated, and a compromised service can use its own token ([0016](0016-risk-register.md)).
