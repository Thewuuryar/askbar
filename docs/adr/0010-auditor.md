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
| Admin read token | Read logs and set retention, never below 7 days | Bootstrap | Admin |
| Audit passphrase | Unseal content | The owner, at first run | Only the owner |

- A normal admin config push can't replace these tokens.
- The auditor accepts appends only, and uses a hash chain.
- When the auditor is down or full, model work fails closed. Reserve disk space, alert early, and recover from the host.
- Sealed audit content is retained for 30 days by default, with a 7-day minimum.

## Alternatives considered

Not recorded.

## Consequences

If the auditor is down, model work stops. Records sent after a server compromise can be false or missing, so [0016](0016-risk-register.md) adds independent records and alerts.
