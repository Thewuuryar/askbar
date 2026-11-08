# 0050. A `CHOWN` exception for `bootstrap`

Status: Accepted
Date: 2026-11-08
Amends: [0032](0032-container-hardening.md), the service exceptions and checks (rules 3 to 6)
Source: owner review of the open questions in [0049](0049-bootstrap.md) (2026-11-08)

## Context

[0032](0032-container-hardening.md) rule 1 gives every service `cap_drop: [ALL]`, rules 3 to 5 list the services that differ, and rule 6 names the check. `bootstrap` ([0049](0049-bootstrap.md)) must write token and hash files that several other uids read, and on an existing install it must write into `server-token`, which belongs to the admin's uid. With every capability dropped, no single uid can do that.

## Decision

0032's rules 3 to 6 are replaced by these, in full:

3. **Caddy:** it binds ports 80 and 443 with `NET_BIND_SERVICE` or the unprivileged port setting. It keeps only what that needs.
4. **`cpa`:** it uses these settings where it allows them. Any exception, such as a writable config path, is documented after testing.
5. **Server:** remove `host.docker.internal:host-gateway`. If the mock model needs it, keep it in a development override only.
6. **`bootstrap`:** it runs as root with `cap_add: [CHOWN]` and no other capability, so it can give each file it writes to its reader. It has no network, mounts only credential volumes, and keeps every other default of rule 1. Under rule 2 the credential volumes count as its own: it writes them, and each service then reads its own ([architecture.md](../architecture.md), Volumes).
7. **Checks:** `check-isolation.sh` confirms each service's settings, including that `bootstrap` has `CHOWN` and no other capability, and no network.

## Alternatives considered

- **One `bootstrap` run per reader uid, or setgid directories seeded from the image:** rejected in [0049](0049-bootstrap.md). Neither can write into the existing `server-token` directory on an upgrade.
- **`DAC_OVERRIDE` instead of `CHOWN`:** rejected. It would let `bootstrap` read and write any file on its volumes without changing owners, and the files would stay owned by root, so readers would need wider modes.

## Consequences

`bootstrap` is the second service, after Caddy, that adds a capability. It holds it only for the seconds it runs, with no network. `check-isolation.sh` checks that `CHOWN` is its only capability.
