# 0032. Container hardening

Status: Accepted
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
6. **`bootstrap`:** it runs as root with `cap_add: [CHOWN]` and no other capability, so it can give each file it writes to its reader ([0049](0049-bootstrap.md)). It has no network, mounts only credential volumes, and keeps every other default of rule 1. Under rule 2 the credential volumes count as its own: it writes them, and each service then reads its own ([architecture.md](../architecture.md), Volumes).
7. **Checks:** `check-isolation.sh` confirms each service's settings, including that `bootstrap` has `CHOWN` and no other capability, and no network.

## Alternatives considered

- **Hardening only the highest-risk services:** rejected, because any compromised service could then attack its neighbors more easily.
- **For `bootstrap`, one run per reader uid, or setgid directories seeded from the image:** rejected. Neither can write into the existing `server-token` directory, which belongs to the admin's uid, on an upgrade.
- **For `bootstrap`, `DAC_OVERRIDE` instead of `CHOWN`:** rejected. It would let `bootstrap` read and write any file on its volumes, and the files would stay owned by root, so readers would need wider modes.

## Consequences

Each service must work with a read-only root filesystem and limited memory, so limits need testing on the real host. A setting doesn't count until the stack has run with it. Hardening doesn't stop a kernel or container escape, which is an accepted risk ([0016](0016-risk-register.md)). `bootstrap` is the second service, after Caddy, to add a capability; it holds it only for the seconds it runs, with no network.

## Amendments

- 2026-11-08: rule 6, a `CHOWN`-only exception for `bootstrap`, and rule 7, its check ([0050](0050-bootstrap-chown-exception.md)).
- 2026-11-10: merged in 0050; the rules above are how they stand.
