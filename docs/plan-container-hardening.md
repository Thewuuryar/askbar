# Plan: harden the containers

Status: tracker for hardening work not yet decided or built. The v1 container settings are decided in [ADR 0032](adr/0032-container-hardening.md). The egress proxy ships first after v1 ([ADR 0006](adr/0006-v1-outbound-access-and-alerts.md), [ADR 0019](adr/0019-post-v1-order-and-tools.md)).

## Why

The owner's priority is a compromised server, then a compromised admin, and keeping every path a model or a bug could use to break containment short. On `main`, only the controller is hardened in `compose.yaml` today (`read_only`, `cap_drop: [ALL]`, `no-new-privileges`), and v1 drops the controller ([ADR 0005](adr/0005-no-controller-and-self-restart.md)).

## Proposals

Each is small enough to ship alone. Order is by value for effort.

1. **Egress proxy, after v1.** In v1, Caddy, `model-gateway`, `cpa` and `alerter` have direct outbound access and the server has none ([ADR 0006](adr/0006-v1-outbound-access-and-alerts.md)). Docker has no built-in egress filter, so the proxy denies by default and allows destinations per source ([ADR 0019](adr/0019-post-v1-order-and-tools.md)). It needs its own name and ADR first ([ADR 0021](adr/0021-names-and-new-components.md)). Still open: how its allowed list follows the admin's provider settings, and who maintains it.
2. **Treat the server's replies as untrusted in the admin.** Check size and shape of every response the admin reads from the server API, and keep rendering them with `textContent`.
3. **Independent records for investigating a compromise.** After a compromise the server can lie. Keep records the server does not control and send them to the auditor with their own append-only credentials. In v1 these are `model-gateway`'s call records ([ADR 0030](adr/0030-sealed-audit-content.md)) and Caddy's access log ([ADR 0016](adr/0016-risk-register.md)). After v1 the egress proxy's log adds what went out, to which host, and how many bytes. A reviewer can then check the server's account against them.
4. **An off-host copy of the audit store**, such as the NAS or object storage with object lock, if a compromised host is in scope. ADRs 0006 and 0016 accept this risk for v1 and defer a dead-man monitor and an off-host chain head. Needs a decision on where, and on how the auditor reaches it, since it has no outbound route.

## Open questions

- Who maintains the egress proxy's allowed list (item 1)?
- Do any services still need a writable root filesystem? [ADR 0032](adr/0032-container-hardening.md) allows a documented exception for `cpa` after testing.
