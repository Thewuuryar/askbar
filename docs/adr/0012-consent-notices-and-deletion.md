# 0012. Consent, notices, and deletion

Status: Accepted 
Date: 2026-11-04
Plan decisions: 22; client warning from 16

## Context

Users should know what's recorded and who can open it. Deployers run Askbar in different settings and have their own legal duties.

## Decision

- **Consent:** provisioning a user in the admin assumes that user's consent.
- **Deployer duty:** the docs state that each deployer is responsible for consent, notice, and legal duties, which can differ for minors, employees, and other countries.
- **Privacy notice:** it is on by default at pairing and whenever the relevant settings change. It says what Askbar records, who can open it, and how long it's kept, and it shows the retention days. Its version is a hash of its text and the retention, so it is shown again whenever either changes. It reaches the client as a policy in the snapshot. The admin can change the wording or turn it off. Because provisioning a user assumes consent, the client records no acknowledgment (from plan step 5.1).
- **AI warning:** clients show "AI can make mistakes. Check important information." above the chat box by default (that text is the default). It reaches the client in the same policy. The admin can change the wording or turn it off.

| Data | When it goes |
| --- | --- |
| Personal space | When the admin deletes the user: the server removes the personal space file, the user's clients and their token hashes, and the user row. Audit records stay. The admin types the user's name to confirm, after a fresh sign-in, in a browser session only (from plan step 5.1) |
| Shared space | When the admin deletes the space, after a confirmation |
| Sealed audit content | When audit retention expires: 30 days by default, 7 at minimum |
| Backups | When backup retention removes them: by default, more than 5 files or older than 30 days, and the newest backup is always kept |

## Alternatives considered

Not recorded.

## Consequences

Deleted data survives in backups and audit records until those periods end. A notice that the admin has turned off gives users no warning.

## Amendments

- 2026-11-11: what the notice shows and deleting a user (#70, owner's decisions D15 and D16, decision changes): the notice shows the retention days and has a version (a hash of its text and the retention); the AI warning's text is the default here; both reach the client as a policy; no acknowledgment is recorded; deleting a user removes the personal space file, the clients, their token hashes and the user row, and leaves audit records, with a typed-name confirmation after a fresh sign-in. Design for plan step 5.1; the code PRs add the 'built' line.
- 2026-11-12: deleting a user is built (plan step 5.1, PR AB). Built so: `DELETE /api/server/v1/users/{id}` removes the personal space file (and its `-wal` and `-shm`), the clients, the hashes of their tokens and the user row in one transaction that is rolled back if a file cannot be removed, and stops replies running in that space first; a space shared with others stays, with what the user wrote in it, and they stop being a member. The admin's `DELETE /api/admin/v1/users/{id}` is session only, needs a sign-in from the last 10 minutes, calls the server first, then removes its own rows for the person and pushes the snapshot, and is recorded `users` / `delete`. A server answer of 404 counts as done, so a delete that failed halfway is finished by doing it again. The Clients page asks for the name to be typed. Not built: the privacy notice and the AI warning (PR AC).
