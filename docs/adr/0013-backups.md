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
- 2026-11-12: built the backup key (plan step 6.1, PR AI, #86). Built so: `askbar-service`'s `backup` feature uses `age` 0.11 (default features off; 0.12 pulls `crypto-bigint` 0.5.5, RUSTSEC-2026-0387, through P-256 support we never call; `deny.toml` carries one dated ignore, RUSTSEC-2026-0173, the unmaintained compile-time macro crate `proc-macro-error2`). `backup.rs` makes an X25519 key pair and protects the private half with the passphrase (scrypt, work factor 2^18 fixed, a file asking for more than 2^22 refused), so the protected file is a plain age file the ordinary `age` tool opens; the fingerprint is the first 16 hex digits of the SHA-256 of the public key, in four groups. The admin's `backup_keys` table (migration 20) holds each pair, the public key and the protected private key; the passphrase is stored, logged and recorded nowhere. `PUT /setup/backup-key` and `POST /backup/rotate-key` are session-only (an API key gets 401) and need a sign-in within 10 minutes; they refuse a passphrase under 20 characters, one not repeated, and any audit passphrase (every audit key is tried, the retired ones too); a key the server cannot be given is not kept; the action record names the new key, and the retired one, by fingerprint. The old passphrase is not asked for when rotating, so there is no way to guess it through the route. The public key is in the server's snapshot (`backup_public_key`) and the alerter's settings, which the admin pushes again at once; the auditor and `cpa` follow in plan steps 6.4 and 6.3. The setup wizard asks for the passphrase after the audit key, and an install from before it is asked on its next visit; the Backup page shows the fingerprint and rotates the key.
- 2026-11-12: built the encrypted exports of the server and the admin and the manual bundle (plan step 6.2, PR AJ, #87). Built so: the server's `POST /exports` writes an age file to the backup public key in its snapshot, streamed (each database copied with `VACUUM INTO` into `<data>/.export-<id>/`, never `/tmp`, one at a time, removed however the export ends, stale ones at the next export), complete with its secrets because it is encrypted, and `503 backup_key_missing` (a new error code, in every OpenAPI file) with no key; it has no length, so a transfer that ends early ends the connection badly, which is how the admin knows it is not whole. The admin's own export is `admin.age`: `admin.db` (a `VACUUM INTO` copy), `admin-secrets/` (`admin.key`, `cpa-api.key`, `cpa-control.secret`) and `caddy-secrets/` (`self.crt`, `self.key`). `GET /backup` is now the bundle, a tar with `manifest.json` (format 1, release, key fingerprint, each part's SHA-256 and size), `backup-key.age`, `admin.age`, then `server.age` (the alerter's, `cpa`'s and the auditor's, the auditor's last, follow in plan steps 6.3 and 6.4), made from work files on the admin's data volume and streamed with a known length; the admin counts and hashes the bytes and cannot open them. **The contract changed:** the manual backup used to be a plain tar with the secrets taken out; it is now encrypted and holds them, and needs a backup key. Verified with the real `age` 1.1.1 command-line tool (`tests/e2e/age_interop.py`): the passphrase opens `backup-key.age`, and the key opens each part.
- 2026-11-12: built the alerter's and `cpa`'s exports (plan step 6.2, PR AK). Built so: the alerter's `POST /exports` (admin link only) is a tar of `ledger.json` and `releases.json` encrypted to the backup public key it was pushed (`503 backup_key_missing` without one or with one that is not a public key); `cpa`'s supervisor answers an `export-request` (`{id, recipient}`) by encrypting the regular files in `cpa-auth` to that key into `export.age` and writing `export-done` (`{id, ok, bytes}`); the admin adds `alerter.age` then `cpa.age` after `server.age`, so the bundle order is admin, server, alerter, `cpa` and, in PR AL, the auditor last. Each must be made or no bundle is sent (`502`, or `503 backup_key_missing` while the alerter has not been given the key yet); a part is left out only where the install has no alerter link or no managed `cpa`, which a stack always has. The supervisor and the alerter now link `askbar-service` with the `backup` feature (the supervisor also takes `serde_json`, already in the lock file); neither adds a crate to `Cargo.lock` beyond what `age` brought in AI.
- 2026-11-12: built the auditor's export (plan step 6.2, PR AL). Built so: on `admin-auditor-read` the auditor takes `PUT /backup-key` (`{public_key}`; refused with `400` unless it is an `age1…` key; kept as a setting of the store, and the same key again records nothing) and answers `GET /export`: a tar with one file, `audit.db`, a `VACUUM INTO` copy taken from a read-only connection (the writer is never blocked) and encrypted with age to that key as it is streamed, with no length, so a transfer that ends early ends the connection badly. Work files are in `.export-<id>/` next to `audit.db` on the audit volume, one export at a time, removed however it ends (stale ones at the next export). `503 backup_key_missing` without a key; `503 audit_backlog`, and nothing made, when free space is below the size of the store plus the floor kept for open spans, or another export is running. The admin gives the auditor the key with each export (idempotent), so a rotation or an auditor that lost its volume leaves no old key behind, then stages `audit.age` last: the bundle order is now admin, server, alerter, `cpa`, auditor, as decided. The auditor is no longer outside the bundle: the guide's separate volume copy is now an extra. `askbar-auditor` links `askbar-service` with the `backup` feature; no new crate. The restore into an empty store is PRs AM and AN.
- 2026-11-12: built the restore of everything but the auditor's store (plan step 6.3, PR AM). Built so: `POST /restore/inspect` (the bundle as a stream, kept as ciphertext in `.restore-upload-<id>/` on the admin's volume for 30 minutes, one at a time) checks the manifest and every part's hash and refuses a newer release (`409 backup_newer_release`, with `release`) before the passphrase is asked for; `POST /restore` takes `{restore_id, passphrase}`, needs a fresh sign-in (both are session-only), is limited by `audit_key::Limiter` (five wrong in fifteen minutes lock it for fifteen), opens the protected key in memory, reads every part to the end before touching any component, then gives the server `POST /imports` (streamed, decrypted as it goes), the alerter `POST /imports`, `cpa` its supervisor's `import-request`, and last writes the admin's own export to `.restore-pending/` next to `admin.db` (opening it once, so a newer admin schema is refused before the restart). `restore::apply_pending` swaps it in at the next start, before the database is opened: secrets first, then `admin.db`, the old ones kept as `*.before-restore`; `main` then sends `restore_performed` once. An install is empty for the admin when it has no provider, person, API key or audit key; the server's `/imports` is `409 not_empty` with more than its seeded owner, a paired client, a pairing code or a conversation, and swaps hot (SQLite's backup API into the open connection, the space files moved in, no restart); the alerter's is `409 not_empty` once it has kept a repeat count or a release result. `backup_newer_release` and `not_empty` are new error codes in every OpenAPI file. A step that fails after others worked names them in `restored`. The auditor's part (`audit.age`) is verified and not restored: PR AN adds the restore link. Two new feature flags on crates already in the lock file: `rusqlite/backup` in the server, `reqwest/stream` in the admin (the admin also lists `futures-util`).
- 2026-11-12: built the auditor's restore (plan step 6.3, PR AN). Built so: the restore now covers every part. After the server, the alerter and `cpa`, the admin streams the decrypted `audit.age` to the auditor's `POST /restore` over `admin-auditor-restore` (the token is read at start like the others), keeps the head it answers with as the one it expects (live and in its pending set), and only then writes its own part; the answer lists `auditor` in `restored` and `not_restored` is empty unless the install has no link for a part. A failure at the auditor names what was restored before it, as any step does.
