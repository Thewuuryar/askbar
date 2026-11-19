---
id: settings/security
title: Security
summary: How you sign in to the admin, the keys scripts use, how long audit records are kept and sealed content stays open, rotating the audit and backup keys, and the kill switch.
keywords: [security, password, passkey, recovery codes, admin API key, sealed content, audit key, audit retention, unseal, backup key, kill switch, danger zone]
settings: [security]
---

# Security

How you sign in to this admin, keys for scripts, how long audit records are kept and sealed content stays open, and the kill switch. Sealed content is the prompts and replies in the audit store, which open only with the audit passphrase; the guide explains how that works in [The Audit page](../../guide.md#the-audit-page). The meaning of *admin* and *admin API key* is in the [glossary](../../architecture.md#glossary).

## Change admin password

Choose a new password for this admin. Other signed-in browsers are signed out.

## Passkey

Sign in with a fingerprint, face or security key.

## Recovery codes

For when you lose your password or passkey.

## Admin API key

For scripts: send it as `Authorization: Bearer <key>` to `/api/admin/v1`. It can do what this site can, except manage keys, change the password or sign in.

## Sealed content and audit records

How long audit records are kept before they are deleted (7 days at least, always; the audit database itself refuses to delete anything younger), and how long sealed content stays open once the audit passphrase has unsealed it: it seals itself after this many minutes without use, or this many in all, whichever comes first. Changing either is an alert. Lowering the retention needs a recent sign-in. These have their own Save; saving General does not touch them.

## Rotate the audit key

Make a new audit key with a new passphrase. Content sealed before stays sealed with the old key and still needs the old passphrase. Neither passphrase can be recovered.

## Backup key

Only the backup passphrase opens a backup. It is not the audit passphrase. Rotating needs a recent sign-in, and backups made before still open with the passphrase they were made under. Backups themselves are on the Backup page.

## Danger zone

The kill switch. Switched on, the server starts no new replies and clears the messages waiting in queues. Clients see a clear message. The box above the switch, **Also stop the replies that are running**, is a setting of its own: tick it and the Save bar appears, and Save stores it without touching the switch. The switch uses the stored choice when it goes on, and its confirmation says what will happen to the running replies; a tick that is not saved yet is not used.
