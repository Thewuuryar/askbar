# 0013. Backups

Status: Accepted
Date: 2026-11-04
Plan decisions: 9

## Context

One file and one passphrase should restore a deployment, while the admin stays unable to read exported data.

## Decision

V1 has manual and scheduled backups, encrypted with age.

**Encryption**

1. At setup, the owner chooses a passphrase of at least 20 characters, separate from the audit passphrase. It can't be recovered if lost.
2. The admin creates a key pair and encrypts the private key with the passphrase.
3. The admin discards the passphrase and gives the public key to each recipient: the server (in the snapshot), the alerter (in `PUT /config`), the auditor (`PUT /backup-key` on its read link) and `cpa` (in its request file). Setting or rotating the key needs a sign-in within the last 10 minutes.
4. Each backup includes the protected private key.
5. Each public key change is recorded in the audit log, and the auditor sends the `backup_key_changed` alert when it takes the new key ([0055](0055-alerter.md)).

**Rotation**

- Rotating the audit passphrase doesn't affect backups or restore. Backups hold sealed content as ciphertext, and content sealed under an old passphrase still needs that passphrase ([0011](0011-sealed-content-access.md)). A restore never needs an audit passphrase.
- Rotating the backup passphrase makes a new key pair. Each backup carries the key protected by the passphrase in force when it was made, so older backups need the old passphrase.
- After a backup passphrase rotation, the admin takes a new backup at once. Keep older backups only until the new ones cover the retention window, and delete them if the rotation followed a suspected compromise.

**Flow and restore**

1. Each component encrypts its snapshot to the public key and exports it through its own authenticated API: `POST /exports` on the server and the alerter, and `GET /export` on the auditor's read link (read-only, from a `VACUUM INTO` copy). The admin exports itself: `admin.db`, `admin.key`, the `cpa` secrets, and the certificate and key from `caddy-secrets`. `model-gateway` has no export: it holds nothing on disk, and the admin pushes its configuration again after a restore. `cpa` has no export route, so the admin writes a request file to `cpa-config` and the supervisor exports and imports its auth files ([0037](0037-askbar-cpa-image-and-supervisor.md)); there is no port.
2. The admin bundles the encrypted exports and adds a manifest of their hashes, the Askbar release version, the key's fingerprint and the protected private key (`backup-key.age`). The admin's own export goes first in the bundle and the auditor's last. The admin can't read the others, and it never mounts another component's volume.
3. A restore decrypts the exports only in an empty, unconfigured install, after a sign-in within the last 10 minutes, and in memory. The server and the alerter take theirs through `POST /imports`, refused with `409 not_empty` unless the install is empty. The auditor takes its store through a link of its own, `admin-auditor-restore` (`POST /restore`), which works only on an empty store and runs the chain check first: this is the one way records reach the auditor other than an append ([0010](0010-auditor.md)). The admin then re-anchors the chain head it stores, so the restore raises no false rollback alert. The admin restores itself last: a pending set, a restart, then the swap. It records `backup/restore` and the admin sends `restore_performed`.
4. A restore checks the release version before asking for the passphrase. It refuses a backup from a newer release and names the release to install. After 1.0, an older backup restores into a newer release and then runs the normal migrations.

| Included | Excluded |
| --- | --- |
| The control database and every space file | Container logs |
| Admin data, including secrets | The audit passphrase |
| The `cpa` configuration and subscription credentials | |
| Audit records: metadata, sealed content as ciphertext, and the hash chain | The backup passphrase |
| Audit-bound logs, such as settings and policy changes | |
| The `alerter` state and delivery settings | |

**Destinations**

| Backup type | Browser download | `/backup` volume |
| --- | --- | --- |
| Manual | Yes | Yes |
| Scheduled | No | Yes |

- One `backup` volume, which only the admin mounts, at `/backup`. To keep backups on a NAS, the host mounts the share over the volume's directory; there is no Docker SMB or NFS volume, so the admin needs no route to the local network and no mount privileges.
- The admin shows whether `/backup` is writable, and warns if it is on the same disk as Askbar (`st_dev`).
- The share's credentials stay in the host's mount configuration, never in Compose, `.env` or the admin.
- Every filesystem call on `/backup` is time-boxed, and a test shows an unavailable destination does not keep the admin down.

**Schedule, retention, and testing**

- The schedule is daily at 03:30 UTC, which meets the data loss target. A backup is written to a temporary file and renamed, so it is all or nothing.
- Delete a backup once there are more than 5 files or it's older than 30 days, and always keep the newest backup. The admin can change both limits.
- The admin sends `backup_failed` for a failed backup and `backup_missed` when none succeeded within 26 hours ([0055](0055-alerter.md)).
- CI runs a restore test with a throwaway key and no audit passphrase, including a test that a backup from a newer release is refused.
- A person runs a restore drill before 1.0 and before each release with a migration. The admin shows a reminder every 90 days, with a 'drill done' action.

## Alternatives considered

- **Keeping secrets out of backups:** rejected, because one file should restore everything.
- **A passphrase typed for each backup:** rejected, because it prevents scheduled backups.
- **A passphrase stored on the admin:** rejected, because a compromised admin could open every backup.
- **An offline private key file:** rejected in favor of one passphrase.
- **The admin encrypting the bundle:** rejected, because it would see each export unencrypted.
- **A `backup-sender` container:** rejected in favor of one volume that the host may mount over a share.
- **A Docker SMB or NFS volume:** rejected (2026-11-11). Its credentials would sit in Compose, and the host's mount is simpler.
- **A gateway export:** rejected. `model-gateway` holds no data.

## Consequences

A stolen backup can be attacked offline, so the passphrase length matters. A lost passphrase makes backups unusable. A backup needs the backup passphrase that was current when it was made. A share on the same site doesn't protect against a fire or theft that affects both machines.

## Amendments
- 2026-11-11: the alerter's backup holds its state (the repeat ledger and release results), not delivery settings: the admin keeps the delivery secrets and pushes them, so they come back with the admin's own backup ([0055](0055-alerter.md)).
- 2026-11-11: decisions of the owner (#70, D21 to D29): the public key goes to each recipient by its own route; `model-gateway` has no export; `cpa` uses request files; the bundle order; the auditor restores through `admin-auditor-restore` into an empty store and the admin re-anchors its chain head; one `backup` volume with the NAS mounted by the host, so the share-volume column, the share test and Docker SMB credentials are gone; the schedule is daily at 03:30 UTC with a 26-hour missed rule; each alert has a named sender; setting the key and restoring need a fresh sign-in.
