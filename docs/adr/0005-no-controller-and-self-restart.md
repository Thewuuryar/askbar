# 0005. No controller, with self-restart

Status: Proposed
Date: 2026-11-04
Plan decisions: 12; Drop the controller; Use self-restart endpoints

## Context

Docker socket access could mount any volume, which would bypass storage isolation. Routine restarts should still work from the admin.

## Decision

Neither v1 nor v2 has a controller, and no service gets the Docker socket for restarts. The admin asks a service to exit cleanly, and Docker's restart policy starts it again.

| Service | Restart approach |
| --- | --- |
| `server`, `admin`, `model-gateway` | Built-in self-restart endpoint |
| `caddy` | Live config reload, with no custom endpoint |
| `cpa` | Test its reload behavior first, and add a small supervisor process only if needed |
| `auditor` | Host recovery only |

Each endpoint has authentication, an audit event, a cooldown, a shutdown limit, and restart-loop alerts. Services the project builds also get an internal watchdog, because Docker marks a failed health check unhealthy but doesn't restart the container.

## Alternatives considered

- **A controller with the Docker socket:** rejected, because its access could get around storage isolation.
- **Restarting only from the host:** rejected. It removes routine recovery from the admin.

## Consequences

A hung service, a failed admin, or the auditor needs host recovery. The admin also can't pull images, so updates run from the host ([0017](0017-dependencies-and-patches.md)). Docker, and any DNS or router settings, must exist before deployment.
