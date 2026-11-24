# 0006. V1 outbound access and alerts

Status: Accepted
Date: 2026-11-04
Plan decisions: 3, 33; Keep the egress proxy out of v1

## Context

V1 needs provider access, certificates, and alerts, but the egress proxy, a dead-man monitor, and an off-host audit hash are deferred to keep v1 small. The auditor should still stay offline.

## Decision

- **Direct outbound:** Caddy keeps its direct route, for certificates. `model-gateway`, `cpa` and `alerter` have no direct route out: their traffic goes through the egress proxy ([0056](0056-egress-proxy.md)).
- **No outbound:** the server and auditor have no outbound route.
- **Egress proxy:** shipped, as the post-v1 step in [0019](0019-post-v1-order-and-tools.md). It denies by default and allows per source and host. `cpa` reaches only Anthropic and OpenAI hosts.
- **Alerter:** it delivers alerts through ntfy or email, so the auditor stays offline. Both go through the egress proxy, mail by its SMTP relay. No exception to this ADR is needed.
- **Not in v1:** a dead-man monitor and an off-host audit hash. Both go on the backlog for after v1 ([0016](0016-risk-register.md)).
- **Emergency use:** when the owner is away, alerts reach their phone, and the stop script remains the emergency control.

## Alternatives considered

- **Sending alerts directly from the auditor:** rejected, because the auditor stays offline.
- **The egress proxy, a dead-man monitor or an off-host audit hash in v1:** rejected to keep v1 smaller.

## Consequences

A compromised `model-gateway`, `cpa` or `alerter` reaches only the hosts its proxy allowlist names ([0056](0056-egress-proxy.md)). Without the monitor and the off-host hash, nothing outside the host notices a silent system or a rewritten audit chain. These risks are accepted for v1 ([0016](0016-risk-register.md)).

## Amendments

- 2026-11-10: two alternatives with the same reason are one line. No decision changed.
- 2026-11-24 (proposed, needs the owner's approval in this pull request): the egress proxy shipped ([0056](0056-egress-proxy.md)). Direct outbound ended for `model-gateway`, `cpa` and `alerter`; Caddy keeps its route for certificates. The alerter's mail goes through the proxy's SMTP relay, with no exception to this ADR. `cpa` reaches only Anthropic and OpenAI hosts, and the proxy denies by default. Reason: the proxy is now the one route out for those services, so the body describes that state.
