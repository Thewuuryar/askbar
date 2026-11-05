# 0010. Auditor

Status: Accepted 
Date: 2026-11-04
Plan decisions: 10, 11

## Context

Audit records must survive a compromised server or admin, and their credentials must have narrow permissions.

## Decision

| Credential | Allows | Set by | Held by |
| --- | --- | --- | --- |
| Service append token | Add records only; no read, change, or delete | Bootstrap | Each sending service |
| Admin append token | Add admin action records from a fixed event list only; no read, change, or delete | Bootstrap | Admin |
| Admin read token | Read logs and set retention, never below 7 days | Bootstrap | Admin |
| Audit passphrase | Unseal content | The owner, at first run | Only the owner |

- A normal admin config push can't replace these tokens.
- Only `model-gateway` opens model-call spans ([0031](0031-audit-span-start-and-backlog.md)). The server can add records to an open span but can't open one.
- The admin's events are settings, restarts, configuration pushes, and secret, retention, and backup key changes, plus sealed-content sessions ([0011](0011-sealed-content-access.md)). The admin refuses an action if its record can't be appended.
- Admin records are claims. An admin action with no matching record from the service that applied it ([0028](0028-service-boundaries-and-config-delivery.md)) raises an alert.
- The auditor accepts appends only, and uses a hash chain.
- When the auditor is down or full, model work fails closed. Reserve disk space, alert early, and recover from the host.
- Sealed audit content is retained for 30 days by default, with a 7-day minimum.

## Alternatives considered

Not recorded.

## Consequences

If the auditor is down, model work stops. Records sent after a server or admin compromise can be false or missing, so [0016](0016-risk-register.md) adds independent records and alerts.
