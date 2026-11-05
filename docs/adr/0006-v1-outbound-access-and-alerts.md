# 0006. V1 outbound access and alerts

Status: Proposed
Date: 2026-11-04
Plan decisions: 3, 33; Keep the egress proxy out of v1

## Context

V1 needs provider access, certificates, and alerts, but the egress proxy, a dead-man monitor, and an off-host audit hash are deferred to keep v1 small. The auditor should still stay offline.

## Decision

- **Direct outbound:** Caddy, `model-gateway`, `cpa`, and the small `alerter` use direct outbound access in v1.
- **No outbound:** the server and auditor have no outbound route.
- **Egress proxy:** it stays out of v1 and ships first after v1 ([0019](0019-post-v1-order-and-tools.md)).
- **Alerter:** it delivers alerts through ntfy or email, so the auditor stays offline.
- **Not in v1:** a dead-man monitor and an off-host audit hash. Both go on the backlog for after v1 ([0016](0016-risk-register.md)).
- **Emergency use:** when the owner is away, alerts reach their phone, and the stop script remains the emergency control.

## Alternatives considered

- **Sending alerts directly from the auditor:** rejected, because the auditor stays offline.
- **Including the egress proxy in v1:** rejected to keep v1 smaller.
- **Including a dead-man monitor and an off-host audit hash in v1:** rejected to keep v1 smaller.

## Consequences

A compromised `model-gateway` or `cpa` can send data directly until the egress proxy ships. Without the monitor and the off-host hash, nothing outside the host notices a silent system or a rewritten audit chain. These risks are accepted for v1 ([0016](0016-risk-register.md)).
