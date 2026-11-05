# 0013. Backups

Status: Proposed
Date: 2026-11-04
Plan decisions: 9

## Context

One file and one passphrase should restore a deployment, while the admin stays unable to read exported data.

## Decision

V1 has manual and scheduled backups, encrypted with age.

**Encryption**

1. At setup, the owner chooses a passphrase of at least 20 characters, separate from the audit passphrase. It can't be recovered if lost.
2. The admin creates a key pair and encrypts the private key with the passphrase.
3. The admin discards the passphrase and gives the public key to each component.
4. Each backup includes the protected private key.
5. Each public key change is recorded in the audit log and sends an alert.

**Flow and restore**

1. Each component encrypts its snapshot to the public key and exports it through its own authenticated API. The auditor's export is read-only.
2. The admin bundles the encrypted exports and adds a manifest of their hashes and the Askbar release version. It can't read them, and it never mounts another component's volume.
3. A restore decrypts the exports only in an empty, unconfigured install.
4. A restore checks the release version before asking for the passphrase. It refuses a backup from a newer release and names the release to install. After 1.0, an older backup restores into a newer release and then runs the normal migrations.

| Included | Excluded |
| --- | --- |
| The control database and every space file | Container logs |
| Admin data, including secrets | The audit passphrase |
| Audit records: metadata, sealed content as ciphertext, and the hash chain | The backup passphrase |
| Audit-bound logs, such as settings and policy changes | |

**Destinations**

| Backup type | Browser download | `/backup` volume | SMB or NFS share volume |
| --- | --- | --- | --- |
| Manual | Yes | Yes | Yes |
| Scheduled | No | Yes | Yes |

- The `/backup` volume, share volume, or both are defined in Compose. Docker mounts them, so the admin needs no route to the local network and no mount privileges. Changing a destination means editing Compose and restarting.
- The admin shows mounted destinations, tests that each is writable, and warns if `/backup` is on the same disk as Askbar.
- SMB credentials stay in a credentials file on the host, never in Compose or `.env`.
- Test whether an unavailable share can keep the admin down before v1 ships.

**Schedule, retention, and testing**

- The admin sets the schedule. A daily schedule meets the data loss target.
- By default, delete a backup once there are more than 5 files or it's older than 30 days, and always keep the newest backup. The admin can change both limits per destination.
- A failed or missed backup sends an alert.
- CI runs a restore test with a throwaway key, including a test that a backup from a newer release is refused.
- A person runs a restore drill before 1.0 and before each release with a migration. The admin shows a reminder every 90 days.

## Alternatives considered

- **Keeping secrets out of backups:** rejected, because one file should restore everything.
- **A passphrase typed for each backup:** rejected, because it prevents scheduled backups.
- **A passphrase stored on the admin:** rejected, because a compromised admin could open every backup.
- **An offline private key file:** rejected in favor of one passphrase.
- **The admin encrypting the bundle:** rejected, because it would see each export unencrypted.
- **A `backup-sender` container:** rejected in favor of Compose-defined volumes.

## Consequences

A stolen backup can be attacked offline, so the passphrase length matters. A lost passphrase makes backups unusable. A share on the same site doesn't protect against a fire or theft that affects both machines.
