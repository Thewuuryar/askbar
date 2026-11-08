# 0032. Container hardening

Status: Accepted; amended by 0050 and 0053
Date: 2026-11-04
Source: required by v1 decisions

## Context

The risk register relies on hardened containers to limit a compromised service, so the settings must be defined and tested.

## Decision

1. **Default settings:** every service uses `read_only: true`, `cap_drop: [ALL]`, `no-new-privileges:true`, a `pids_limit`, a memory limit, and a `tmpfs` for `/tmp`. These live in one shared Compose block, so a new service starts hardened.
2. **Writable paths:** a service writes only to its own volumes.
3. **Caddy:** it binds ports 80 and 443 with `NET_BIND_SERVICE` or the unprivileged port setting. It keeps only what that needs.
4. **`cpa`:** it uses these settings where it allows them. Any exception, such as a writable config path, is documented after testing.
5. **Server:** remove `host.docker.internal:host-gateway`. If the mock model needs it, keep it in a development override only.
6. **Checks:** `check-isolation.sh` confirms each service's settings.

## Alternatives considered

- **Hardening only the highest-risk services:** rejected, because any compromised service could then attack its neighbors more easily.

## Consequences

Each service must work with a read-only root filesystem and limited memory, so limits need testing on the real host. A setting doesn't count until the stack has run with it. Hardening doesn't stop a kernel or container escape, which is an accepted risk ([0016](0016-risk-register.md)).
