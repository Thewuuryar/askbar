# 0005. The Docker socket lives only in the controller

Status: Accepted
Date: 2026-11-03 (backfilled from `docs/plan-admin-managed-setup.md`, D1)

## Context

The admin site should start, stop and restart services. That needs the Docker socket, which is root on the host.

## Decision

A small container, `askbar-controller`, is the only one with the socket. It has no published port, no route out, no capabilities and a read-only filesystem. It answers only the admin, by a token whose hash it holds. It can start, stop and restart `server`, `caddy` and `cpa` of its own Compose project, found by Compose labels, and nothing else: no create, exec, pull, volume or other containers. The admin and the controller are not on the list. Details: [SECURITY.md](../../SECURITY.md), "The Docker socket".

## Alternatives considered

- **Socket in the admin.** Rejected: an admin compromise would equal host root and undo [0003](0003-admin-and-server-isolation.md).
- **`tecnativa/docker-socket-proxy`.** Rejected as the primary design: its rules are per Docker API verb, not per container, so it cannot limit which containers. Kept as a fallback if the controller becomes a burden.

## Consequences

A compromised admin can stop or restart three services but cannot run anything new. A compromised controller is root on the host, so it stays small and can be left out of `compose.yaml`. Widening the allow list needs a new ADR.
