# 0011. Sealed audit content and access

Status: Accepted
Date: 2026-11-04
Plan decisions: 11, 22

## Context

Audit records must support investigations. Reading their content must take an explicit, logged act by the owner, with an extra secret. The auditor, and a compromised admin, must not be able to read it at rest.

## Decision

**Sealing**

1. **How:** the server encrypts prompts, replies and provider errors with a public key made from the audit passphrase (Argon2id derives an X25519 key).
2. **No private key outside the admin's session:** the server and the auditor never hold the passphrase or the private key. The auditor stores only metadata and ciphertext. The server can still read its own space files, so sealing protects the audit copy, not the history.
3. **Metadata stays readable:** time, user, space, providers tried, step timings, tokens and outcome. `model-gateway`'s independent records hold metadata only.
4. **No plain copies:** prompts, replies and provider errors appear in the audit store only as sealed payloads, and never in logs or error text.
5. **Missing key:** until an audit key exists, Askbar answers `503 audit_key_missing` instead of recording unsealed.
6. **Rotation:**
   - A new passphrase seals new content with a new key; older content still needs the old passphrase.
   - Each sealed record carries a key ID, and audit metadata keeps a registry of key IDs with their created and retired dates, so the admin can name the passphrase a record needs.
   - Records aren't re-sealed in v1; old content ages out under retention.
7. **Loss:** a lost passphrase can't be recovered, and content sealed under it is gone for good.

**Opening sealed content.** This is its own permission. In v1 only the single admin has it. Admin API keys and regular users can't open sealed content, and adding admins later won't grant it automatically.

8. **Start:** the audit passphrase, entered after a fresh sign-in (with the passkey if one is set up).
9. **Session:** content stays visible without further prompts until the session ends.
10. **The key** is held in the admin's memory only: never on disk, in logs or in backups.
11. **Ending:** after 15 minutes idle or 1 hour in total by default (the admin can set both, up to 12 hours), or on Seal, sign-out or an admin restart.
12. **Records and alerts:**
    - the start, the end and each item viewed are sent to the auditor;
    - every unseal and every timeout change sends an alert;
    - an unseal is refused if its record can't be written.
13. **Export:** no bulk export in v1.

## Alternatives considered

- **Plaintext content behind an admin gate:** rejected. The gate would be software only, and a compromised admin could skip it. It would also store every conversation a second time in plaintext, backups included.
- **A key held by the server or the admin:** rejected. Compromising either would open all content. A confirmation button instead of a passphrase is rejected for the same reason: the admin would have to store the key.
- **A prompt for each item:** rejected in favour of a timed session.

## Consequences

- A compromised server can't open records it already sealed, but can still read conversations in its space files. The server can't search sealed content.
- A compromised admin during a session can read all sealed content and may capture the key, which could then decrypt content without the normal alert. Rotate the passphrase after any suspected compromise; that protects new content only.
- Tests and `check-isolation.sh` confirm that no unsealed content is recorded.

## Amendments

- 2026-11-10: merged in 0030 (sealed audit content), as "Sealing", rules 1 to 7, with its alternatives and consequences; the title now covers both.
- 2026-11-11: built in plan step 2.8. Built so: a passphrase entered within ten minutes of signing in opens one session, bound to that browser session, whose keys (one for each key in the registry that the passphrase makes) live in the admin's memory only; a passphrase for an older key adds to it without restarting its clock. It ends after 15 minutes idle or an hour in all (Settings, up to 12 hours each, idle at most total), on Seal, on sign-out, when its browser session is gone, or when the admin restarts. Admin API keys cannot start or use one. The start, each item viewed and Seal are recorded before they happen and refused if they cannot be (through the admin's action records, [0040](0040-admin.md)); the end by a timeout or a sign-out is recorded with its reason. An unseal and a change of the timeouts are alerts as a log line and a banner until the alerter exists (phase 4). The passkey of rule 8 waits: v1 has none.
