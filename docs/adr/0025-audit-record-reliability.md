# 0025. Audit record reliability

Status: Accepted
Date: 2026-11-04
Source: recommendation accepted after the clean copy

## Context

A crash or brief outage can leave audit spans unfinished, and local tampering must be noticed.

## Decision

These rules cover the services the project builds that append to the auditor ([0003](0003-v1-service-layout.md)). Caddy's access logs are outside them.

1. **Finish records:** a service queues finish and payload records, and retries them until the auditor acknowledges them.
2. **Boot IDs:** a service announces a new boot ID when it connects. The auditor ends that service's earlier unfinished spans as interrupted.
3. **Chain head check:** the admin stores the latest hash chain head at intervals and checks it against the auditor. A rollback, deletion, or edit appears in the admin and sends an alert.

## Alternatives considered

- **Dropping unfinished records:** rejected, because it would leave gaps in investigations.
- **An off-host copy of the chain head in v1:** rejected to keep v1 smaller ([0006](0006-v1-outbound-access-and-alerts.md)). Without it, only the admin check notices tampering.

## Consequences

Services need a bounded retry queue. A fully compromised admin and auditor, or host root, could still agree on false data. This risk is accepted for v1 ([0016](0016-risk-register.md)).

## Amendments
