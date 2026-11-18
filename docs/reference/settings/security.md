---
id: settings/security
title: Security
summary: How you sign in to the admin, the keys scripts use, how long sealed content stays open, and rotating the audit key.
keywords: [security, password, passkey, recovery codes, admin API key, sealed content, audit key, unseal]
settings: [security]
---

# Security

How you sign in to this admin, keys for scripts, and how long sealed content stays open. Sealed content is the prompts and replies in the audit store, which open only with the audit passphrase; the guide explains how that works in [The Audit page](../../guide.md#the-audit-page). The meaning of *admin* and *admin API key* is in the [glossary](../../architecture.md#glossary).

## Change admin password

Choose a new password for this admin. Other signed-in browsers are signed out.

## Passkey

Sign in with a fingerprint, face or security key.

## Recovery codes

For when you lose your password or passkey.

## Admin API key

For scripts: send it as `Authorization: Bearer <key>` to `/api/admin/v1`. It can do what this site can, except manage keys, change the password or sign in.

## Sealed content

How long sealed content stays open once the audit passphrase has unsealed it. The limits are set in General.

## Rotate the audit key

Make a new audit key with a new passphrase. Content sealed before stays sealed with the old key and still needs the old passphrase. Neither passphrase can be recovered.
