---
id: settings/backup
title: Backup & restore
summary: Encrypted backups of the server's data and the admin's settings: making them, scheduling them, keeping them, and restoring one.
keywords: [backup, restore, schedule, retention, backup key, download, recovery]
settings: [backup]
---

# Backup & restore

Encrypted backups of the server's data and this admin's settings. What to back up, and how recovery works, is in the guide: [Backup and recovery](../../guide.md#backup-and-recovery). The encryption is decided in [ADR 0013](../../adr/0013-backups.md).

## Back up now

Backs up now, then applies the keep limits.

## Scheduled backups

Backups run on a schedule. A failed backup is retried after a short wait and raises an alert.

## How long backups are kept

The newest backup is always kept. Lowering the limits needs a recent sign-in.

## Backup key

Only the backup passphrase opens a backup. It is not the audit passphrase. Rotating needs a recent sign-in, and backups made before still open with the passphrase they were made under.

## Download a backup

An encrypted .tar file, opened with the backup passphrase.

## Restore from a backup

Only for a new, empty install. A restore replaces all of this server's data and this admin's own, and needs a recent sign-in. The admin restarts when it is done; sign in again with the password the backup's install had.
