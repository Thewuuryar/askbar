# 0025. Audit record reliability

Status: Proposed
Date: 2026-11-04
Source: recommendation accepted after the clean copy

## Context

A crash or brief outage can leave audit spans unfinished, and local tampering needs more than one way to be noticed.

## Decision

1. **Finish records:** a service queues finish and payload records, and retries them until the auditor acknowledges them.
2. **Boot IDs:** a service announces a new boot ID when it connects. The auditor ends that service's earlier unfinished spans as interrupted.
3. **Chain head check:** the admin stores the latest hash chain head at intervals and checks it against the auditor. A rollback, deletion, or edit appears in the admin and sends an alert.

The off-host hash from [0006](0006-v1-outbound-access-and-alerts.md) is still required.

## Alternatives considered

- **Dropping unfinished records:** rejected, because it would leave gaps in investigations.
- **Relying only on the off-host hash:** rejected, because the admin check can show a problem directly in the admin.

## Consequences

Services need a bounded retry queue. A fully compromised admin and auditor could still agree on false data, which is why the off-host hash stays.
