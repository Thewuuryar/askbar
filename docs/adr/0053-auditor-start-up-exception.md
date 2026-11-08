# 0053. A start-up exception for the `auditor`

Status: Proposed
Date: 2026-11-08
Amends: [0032](0032-container-hardening.md), the service exceptions and checks (rules 3 to 7)
Source: owner review of the open questions in [0051](0051-auditor.md) (2026-11-08)

## Context

The `auditor` ([0051](0051-auditor.md)) runs as its own uid, 10003. On an existing install the files on `audit-data` belong to the server's uid (10002), because the server kept the store until plan step 2.4. With every capability dropped ([0032](0032-container-hardening.md) rule 1), nothing in the stack can give those files to the auditor, and `bootstrap` mounts credential volumes only ([0049](0049-bootstrap.md)).

## Decision

0032's rules 3 to 7, as [0050](0050-bootstrap-chown-exception.md) left them, are replaced by these, in full:

3. **Caddy:** it binds ports 80 and 443 with `NET_BIND_SERVICE` or the unprivileged port setting. It keeps only what that needs.
4. **`cpa`:** it uses these settings where it allows them. Any exception, such as a writable config path, is documented after testing.
5. **Server:** remove `host.docker.internal:host-gateway`. If the mock model needs it, keep it in a development override only.
6. **`bootstrap`:** it runs as root with `cap_add: [CHOWN]` and no other capability, so it can give each file it writes to its reader. It has no network, mounts only credential volumes, and keeps every other default of rule 1. Under rule 2 the credential volumes count as its own: it writes them, and each service then reads its own ([architecture.md](../architecture.md), Volumes).
7. **`auditor`:** it starts as root with `cap_add: [CHOWN, SETUID, SETGID]` and no other capability. Before it opens the store or a port, it gives the files on `audit-data` to its own uid if they are not already, then switches to that uid, which drops every capability. It keeps every other default of rule 1.
8. **Checks:** `check-isolation.sh` confirms each service's settings, including that `bootstrap` has `CHOWN` and no other capability, and no network, and that the running `auditor` process has no capabilities (`CapEff` is 0).

## Alternatives considered

- **Running the auditor as the server's uid, or a one-time host command:** rejected in [0051](0051-auditor.md).
- **Letting `bootstrap` change the store's owner:** rejected. `bootstrap` would mount a data volume that holds sealed content and metadata, which [0049](0049-bootstrap.md) rules out.
- **`DAC_OVERRIDE` instead of `CHOWN`:** rejected, as in [0050](0050-bootstrap-chown-exception.md). The running auditor would keep it to read files it does not own.

## Consequences

The auditor holds three capabilities only between its start and the switch, before it reads any input. After the first start on an upgraded install the owner change does nothing, and the switch still happens on every start.
