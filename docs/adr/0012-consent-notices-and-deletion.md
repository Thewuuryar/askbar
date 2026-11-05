# 0012. Consent, notices, and deletion

Status: Proposed
Date: 2026-11-04
Plan decisions: 22; client warning from 16

## Context

Users should know what's recorded and who can open it. Deployers run Askbar in different settings and have their own legal duties.

## Decision

- **Consent:** provisioning a user in the admin assumes that user's consent.
- **Deployer duty:** the docs state that each deployer is responsible for consent, notice, and legal duties, which can differ for minors, employees, and other countries.
- **Privacy notice:** it is on by default at pairing and whenever the relevant settings change. It says what Askbar records, who can open it, and how long it's kept. The admin can change the wording or turn it off.
- **AI warning:** clients show "AI can make mistakes. Check important information." above the chat box by default. The admin can change the wording or turn it off.

| Data | When it goes |
| --- | --- |
| Personal space | When the admin deletes the user |
| Shared space | When the admin deletes the space, after a confirmation |
| Sealed audit content | When audit retention expires: 30 days by default, 7 at minimum |
| Backups | When backup retention removes them: by default, more than 5 files or older than 30 days, and the newest backup is always kept |

## Alternatives considered

Not recorded.

## Consequences

Deleted data survives in backups and audit records until those periods end. A notice that the admin has turned off gives users no warning.
