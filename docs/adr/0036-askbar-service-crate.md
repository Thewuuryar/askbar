# 0036. The `askbar-service` crate

Status: Proposed
Date: 2026-11-07
Source: plan step 1.3 ([plan-v1.md](../plan-v1.md)); owner rule that every new component gets an ADR before it is created

## Context

[0028](0028-service-boundaries-and-config-delivery.md) gives every service the project builds the same duties: answer `503 not_configured` until configured, report its configuration version on `GET /status`, and let the admin see a restart. [0004](0004-bootstrap-and-per-link-tokens.md) gives each link its own token, checked against a hash. [0022](0022-api-path-conventions.md) gives them the same internal routes. The server has these today, and the auditor, `model-gateway` and `alerter` will need the same code. Written once per service, the copies would drift, and a security check would be fixed in one copy and not the others.

## Decision

- `crates/askbar-service` is a library crate in the root workspace, used by every service the project builds. It is not a container, holds no data and opens no port of its own.
- It holds only what every service shares:
  - `links`: the per-link bearer check. A link names the caller, the file holding its token's SHA-256, and the routes it may call. A missing, unreadable or malformed hash file accepts nothing. No or a wrong token is `401 unauthorized`; a known token on another link's route is `403 forbidden`.
  - `status`: the boot ID (new on every start) and the `GET /status` body, `{"config_version", "boot_id"}`.
  - `configured`: the gate that answers `503 not_configured` with `Retry-After` until configuration has arrived.
  - `routes`: the common internal paths (`/health`, `/status`, `/config`; `/restart` and `/exports` join when built).
- It depends only on libraries already in the workspace and on `askbar-proto`. It has no admin code, no database code and no provider code, so the server image still carries no admin code ([0028](0028-service-boundaries-and-config-delivery.md) rule 1).
- Anything one service alone needs stays in that service.

## Alternatives considered

- **A copy in each service:** rejected. The token check and the gate are security rules, and copies drift.
- **Putting it in `askbar-proto`:** rejected. `askbar-proto` is also built into the desktop client, which needs no server middleware.
- **A sidecar proxy doing the token check:** rejected. It would add a container per service and a hop on every internal call.

## Consequences

- Every new service starts from the same token check and gate; a fix lands in all of them at once.
- The crate's own tests cover the link rules, the patterns, the boot ID and the gate. Each service still tests its own links and gated routes, and `check-isolation.sh` tests the running stack.
- A change to the crate changes every service, so it needs the same care as a change to a hard rule.
