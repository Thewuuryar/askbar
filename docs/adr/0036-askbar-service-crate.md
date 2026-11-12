# 0036. The `askbar-service` crate

Status: Accepted
Date: 2026-11-07
Source: plan step 1.3 ([plan-v1.md](../plan-v1.md)); owner rule that every new component gets an ADR before it is created
Kind: component

## Context

[0028](0028-service-boundaries-and-config-delivery.md) gives every service the project builds the same duties: answer `503 not_configured` until configured, report its configuration version on `GET /status`, and let the admin see a restart. [0004](0004-bootstrap-and-per-link-tokens.md) gives each link its own token, checked against a hash. [0022](0022-api-path-conventions.md) gives them the same internal routes. The server, the auditor and `model-gateway` use this code, and `alerter` will. Written once per service, the copies would drift, and a security check would be fixed in one copy and not the others.

## Decision

- `crates/askbar-service` is a library crate in the root workspace, used by every service the project builds. It is not a container, holds no data and opens no port of its own.
- It holds only what every service shares:
  - `links`: the per-link bearer check. A link names the caller, the file holding its token's SHA-256, and the routes it may call. A missing, unreadable or malformed hash file accepts nothing. No or a wrong token is `401 unauthorized`; a known token on another link's route is `403 forbidden`.
  - `status`: the boot ID (new on every start) and the `GET /status` body, `{"config_version", "boot_id"}`.
  - `configured`: the gate that answers `503 not_configured` with `Retry-After` until configuration has arrived.
  - `routes`: the common internal paths (`/health`, `/status`, `/config` and `/restart`; `/exports` joins when built).
  - `restart`: self-restart with a cooldown, a shutdown limit and a watchdog ([0005](0005-no-controller-and-self-restart.md)).
  - `audit_client`: the link to the auditor that the server and `model-gateway` share: an acknowledged open, and a bounded queue for everything else ([0010](0010-auditor.md)).
  - **from plan step 4.1,** `alert_client`: the alerter's counterpart of `audit_client`, a bounded queue that sends events to `alerter` over a link ([0055](0055-alerter.md)). `AuditClient` also calls `GET /status` on the auditor every 60 seconds, so the auditor can tell a quiet link from a silent one ([0051](0051-auditor.md));
  - **from plan step 6.1,** a `backup` feature (off by default, on for the services that export) with the `age` crate: encrypt a stream to a public key and parse a recipient. `routes` gains `/exports`, and the server, the alerter and the admin implement it ([0013](0013-backups.md));
  - `ids`, `periods` (the day and month a budget counts over, in UTC) and `tar` (the minimal archive of backup exports).
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

## Amendments

- 2026-11-08: marked as a component ADR, so it is amended in place ([0046](0046-component-and-register-adrs-and-partial-amendments.md)). Nothing else changed.
- 2026-11-11: body brought up to date with the code on `main`: the auditor and `model-gateway` use the crate, and it holds the shared auditor client, self-restart, ids, budget periods and the archive writer. No decision changed (#70).
- 2026-11-11: design for plan steps 4.1, 4.2 and 6.1 (#70): `alert_client`, the 60-second `GET /status` call of `AuditClient`, the `backup` feature with `age`, and `/exports`. The code PRs add the 'built' lines.
- 2026-11-11: built `AuditClient::alert_delivery` and `AuditClient::status_every_minute` (plan step 4.1, PR S, #73). Built so: the alerter's outcomes go to `POST /alert-deliveries` through the same queue as every other record; `status_every_minute` calls the auditor's `GET /status` with the link's own token every 60 seconds from the first moment, needs no boot ID, does nothing when there is no token, and is started by the server, `model-gateway` and the alerter (the admin's own link is PR W). `alert_client` for the admin's events, the `backup` feature and `/exports` are not built yet.
- 2026-11-12: the `backup` feature is built (plan step 6.1, PR AI, #86): `backup.rs`, with `age` 0.11, makes and opens the backup key and encrypts to a recipient; off unless a service asks (the admin does; the server and the alerter will, for their exports). See [0013](0013-backups.md).
- 2026-11-12: `tar` can write to any writer as it reads (`write_file`, `write_bytes`, `write_end`), `backup` has an encrypting stream, a `Manifest` and the bundle's file names, and `stream` is a response body written by blocking code (`ChunkWriter`, which stops when the client goes and ends the connection badly when an export fails) (plan step 6.2, PR AJ, #87). It takes `futures-util`, already in the lockfile.
