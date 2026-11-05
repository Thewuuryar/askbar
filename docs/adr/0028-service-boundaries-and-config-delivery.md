# 0028. Service boundaries and config delivery

Status: Accepted
Date: 2026-11-04
Source: required by v1 decisions

## Context

The server handles untrusted text and must not be able to change settings. `model-gateway` holds keys only in memory, so restarted services need their configuration back with no manual commands.

## Decision

1. **Separate programs:** the admin and server have separate containers, images, user IDs, and volumes. The server image contains no admin code.
2. **One direction:** the server never calls the admin, and never reads or writes the admin's data or settings. A model, tool, or reply can't change settings.
3. **Unconfigured start:** a service that needs configuration answers `503 not_configured` until it has it.
4. **Status and push:** the admin checks each service's `GET /status`, which reports its configuration version, every 10 to 15 seconds. It pushes configuration when a service is unconfigured or out of date, including after a restart. It also pushes as soon as it sees a service's new boot ID in the auditor ([0025](0025-audit-record-reliability.md)).
5. **Records:** each service records the configuration version it applied in the auditor.

## Alternatives considered

- **Services pulling configuration from the admin:** rejected for v1, with no pull for any service. It would give the admin an inbound listener on the internal network, reachable from services such as `model-gateway`, and push with boot detection already restores services without it.
- **Storing secrets in each service:** rejected, because a stolen service volume would expose them. Here, secrets means provider keys and admin data. Each service's own link tokens ([0004](0004-bootstrap-and-per-link-tokens.md)) and the `cpa` subscription credentials ([0014](0014-model-gateway-and-providers.md)) are the exceptions.

## Consequences

After a reboot, services return on their own once the admin is running, which supports the no-commands target. The admin must read its own secrets unattended, so a stolen disk remains an accepted risk unless the host disk is encrypted. `check-isolation.sh` tests these boundaries.
