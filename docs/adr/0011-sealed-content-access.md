# 0011. Sealed content access

Status: Accepted
Date: 2026-11-04
Plan decisions: 11, 22

## Context

The admin must be able to read audit logs and open sealed content, with an extra secret and a clear record of each opening.

## Decision

Opening sealed content is its own permission. In v1, only the single admin has it. Admin API keys and regular users can't open sealed content, and adding admins later won't grant it automatically.

1. **Start:** enter the audit passphrase after a fresh sign-in, using the passkey if one is set up.
2. **Viewing:** all content stays visible without further prompts until the session ends.
3. **Key handling:** keep the private key in admin memory only, never on disk or in logs or backups.
4. **Timeout:** close the session after 15 minutes of inactivity and 1 hour overall by default. The admin can configure both, up to a 12-hour ceiling.
5. **Other endings:** Seal, sign-out, or an admin restart also close the session.
6. **Records and alerts:** send the start, end, and each item viewed to the auditor. Send an alert for every unseal and timeout change. An unseal is refused if its record can't be written.
7. **Export:** leave bulk export out of v1.

## Alternatives considered

- **A confirmation button instead of a passphrase:** rejected, because the admin would have to store the private key.
- **A prompt for each item:** rejected in favor of a timed session.

## Consequences

If the admin is compromised during a session, an attacker can read all sealed content and may capture the key. Rotate the passphrase after any suspected compromise. A captured key could decrypt content without triggering the normal alert.
