# 0002. V1 scope, limits, and ship criteria

Status: Proposed
Date: 2026-11-04
Plan decisions: 1, 14, 15, 17, 23, 27

## Context

V1 must be useful to a family of 3 and fit the owner's available time, budget, and hardware. Hardening never truly ends, so v1 needs a clear security bar.

## Decision

**Scope:** v1 is 1.0. It is conversation only. A fresh deployment and one paired client must work end to end.

**Definition of done:** each step includes tests, docs, a rollback, and failure tests where relevant.

**Limits**

| Limit | Decision |
| --- | --- |
| Owner time | 6 hours per week, about 100 hours to March 1, 2027 |
| Monthly cost | $20 per month on models or an OAuth subscription, with self-hosted models preferred where possible |
| Askbar host | Dedicated headless desktop with 32 to 64 GB of RAM |
| Self-hosted models | A separate desktop with a GPU |
| Backup storage | Most likely an SMB or NFS share from a TrueNAS server |
| Target date | Family using it by March 2027 |

When work doesn't fit, cut scope instead of exceeding a limit. Estimate each v1 item before work starts, then defer in this order: iOS and memory, mismatch alerts, scheduled backups to multiple destinations, then shared spaces.

**Service targets**

| Measure | Project promise | Family target | Test |
| --- | --- | --- | --- |
| Users | No fixed limit; docs state the tested size | 3 active users | 3 simultaneous replies against the mock model |
| Uptime | No guarantee or support | Best effort, and back up after a reboot with no commands | Reboot test |
| Data loss | No guarantee | Up to 24 hours, using a daily backup schedule | Restore drill from a daily backup |
| Recovery from backup | Documented steps | Under 30 minutes on a clean host | Timed drill by a person |
| Service restart | Documented behavior | Under 1 minute from the admin | Scripted restart test |
| Added latency | No guarantee | Under 200 ms at the 95th percentile before the first token, excluding the provider | CI check with the mock model |

The README states that Askbar is an open-source project for personal hardware, with no support or uptime guarantee.

**Ship checklist**

- A fresh deployment and one paired client work end to end, tested by a person.
- A real certificate is issued through this layout.
- `scripts/check-isolation.sh` passes in CI.
- Every risk marked Defend in [0016](0016-risk-register.md) has a passing test.
- The owner has signed off on the accepted risks.
- A person completes a restore drill and meets the recovery target.
- Auditor and admin failure tests pass.
- No known ship-blocking finding remains open.
- Documentation is up to date with v1, including an architecture diagram that matches the shipped services and links.
- The owner has rechecked Anthropic's policy and decided whether to disable Anthropic subscription login for the 1.0 release.
- A person tests the client on macOS, Windows, and Linux.

**Triage rule:** a new finding blocks v1 only if it allows remote access without authentication, exposure of another user's data, exposure of secrets, a bypass of audit recording, or failure of a Defend control. Anything else goes on the backlog with a target version.

## Alternatives considered

Not recorded.

## Consequences

Hardening continues in later versions, and new ideas that don't meet the triage rule don't delay v1. The v1 build still needs an estimate against the 100 hours.
