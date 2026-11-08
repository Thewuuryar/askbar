# 0039. The `server` component

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan step 1.14 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

The server existed before the rule that each component has its own ADR. This record describes it as it is on `main` after plan step 1.11, so the owner can review what is there before phases 2 to 4 change it. The decisions it follows are in [0003](0003-v1-service-layout.md), [0005](0005-no-controller-and-self-restart.md), [0008](0008-users-clients-and-client-access.md), [0009](0009-spaces-and-storage.md), [0022](0022-api-path-conventions.md), [0023](0023-api-error-format.md), [0028](0028-service-boundaries-and-config-delivery.md) and [0032](0032-container-hardening.md); this record links them instead of repeating them.

## Decision

- **What it is:** `services/server`, binary and image `askbar-server`, the Dockerfile's `server` target. It runs as uid 10002 and serves chats for paired clients.
- **Two listeners:**
  - the client API, `/api/client/v1`, on port 8080 of the `edge` network, reached through Caddy;
  - the server API, `/api/server/v1`, on the server's own address on the `server-api` network, found by the alias `server-api-host` that exists on that network only (port 8082), for the admin only. It refuses a wildcard address, or a name that resolves to more than one.
- **What it holds:**
  - `control.db`: users, clients (token hashes only), pairing-code hashes, spaces and memberships, the snapshot the admin pushed (with the budget copy), token usage and provider health;
  - one file per space under `spaces/` ([0009](0009-spaces-and-storage.md));
  - provider API keys, encrypted with its master key;
  - `audit.db`, the audit store, holding sealed content and only the public half of the audit key ([0030](0030-sealed-audit-content.md)).
- **Volumes:** `DATA_DIR` (a host directory, `./data` by default) at `/data`; `askbar-secrets` at `/secrets` (`master.key`); `audit-data` at `/audit`; `server-token` read-only at `/server-token` (the admin's token hash).
- **Networks:** `edge` and `server-api`. It publishes no port.
- **Links:** Caddy calls the client API with a client token. The admin calls the server API with its token, checked against `server-token/token.sha256` through `askbar-service` ([0036](0036-askbar-service-crate.md)). The server calls model providers and `cpa` itself. It never calls the admin.
- **Configuration:** it answers `503 not_configured` until the admin pushes a snapshot, then keeps it in `control.db` ([0028](0028-service-boundaries-and-config-delivery.md)). It restarts itself on `POST /api/server/v1/restart` and when its own health check keeps failing ([0005](0005-no-controller-and-self-restart.md)).
- **Hardening:** the shared defaults of [0032](0032-container-hardening.md), with no exception; memory limit 512 MB.

## Alternatives considered

Not recorded: the server predates this record.

## Consequences

`check-isolation.sh` tests what the server can and cannot reach in the running stack, and the server's own tests cover its APIs, storage and audit trail.

**Where the code and an accepted ADR disagree.** Each is a later plan step, not a decision to keep it:

- [0003](0003-v1-service-layout.md) and [0014](0014-model-gateway-and-providers.md): the server stores provider keys and calls providers itself, and `edge` has an outbound route. In v1, `model-gateway` holds the keys and makes the calls, and the server has no outbound route (plan phase 3).
- [0010](0010-auditor.md) and [0031](0031-audit-span-start-and-backlog.md): the audit store is inside the server, which opens its own model-call spans. In v1 the auditor keeps it and only `model-gateway` opens those spans (phases 2 and 3).
- [0004](0004-bootstrap-and-per-link-tokens.md): the admin makes the server API token; `bootstrap` is not built yet (plan step 2.1).
- [0003](0003-v1-service-layout.md) (no host bind mounts): the data directory is a host bind mount.
- [0013](0013-backups.md): the backup export is not encrypted, so it leaves secrets out (phase 6).
- [0028](0028-service-boundaries-and-config-delivery.md) rule 5: the server does not record the configuration version it applied in an auditor, because there is no auditor yet.

## Amendments

- 2026-11-08: the server API binds the address of the alias `server-api-host` instead of the fixed `172.30.0.2`; no subnet or address is pinned (PR #32, [0051](0051-auditor.md)).
