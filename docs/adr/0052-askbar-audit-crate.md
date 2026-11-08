# 0052. The `askbar-audit` crate

Status: Proposed
Date: 2026-11-08
Kind: component
Source: plan step 2.3 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07); owner review of the open questions (2026-11-08)

## Context

The audit store's code is in the server today: `services/server/src/audit.rs` (the `audit.db` schema and migrations, the writer, the queries and retention) and `services/server/src/seal.rs` (sealing to the audit public key). Plan step 2.3 moves the store into a library, unchanged, so that the `auditor` ([0051](0051-auditor.md)) can run the store in step 2.4. This record describes that crate, and is proposed before step 2.3 so the owner accepts it first. It follows [0010](0010-auditor.md), [0025](0025-audit-record-reliability.md), [0030](0030-sealed-audit-content.md) and [0033](0033-repository-layout.md).

## Decision

- **What it is:** `crates/askbar-audit`, a library crate in the root workspace ([0033](0033-repository-layout.md)), created in the pull request of plan step 2.3. It is not a container and opens no port.
- **What it holds:** the store, moved as it is from `audit.rs`, and nothing else. It has no Cargo features.
  - the `audit.db` store: its own append-only migrations and `PRAGMA user_version`, separate from `control.db` and the space files; never edit an earlier migration (AGENTS.md);
  - the writer: a bounded queue in front of one SQLite connection, with its health and dropped-record count;
  - the queries the admin's audit pages use: traces, spans, sealed payloads and statistics. Statistics take a callback that turns token counts into a cost, so the crate never sees a price type; the caller applies prices;
  - retention, with the 7-day floor (`MIN_RETENTION_DAYS`, [0010](0010-auditor.md));
  - from plan step 2.6, the hash chain and the prune record ([0051](0051-auditor.md)).
- **What it does not hold:**
  - `seal.rs`, which stays in the server, its only user. It seals and nothing more: there is no function that opens a sealed payload, and the server never holds the passphrase or the private key ([0030](0030-sealed-audit-content.md)). Opening stays in the admin.
  - the span record types (start, finish, payload) once they cross the network: in step 2.4 they become the wire types of `/api/audit/v1` and move to `askbar-proto` ([0045](0045-askbar-proto-crate.md)), where every wire type lives. In step 2.3 they move with the store.
- **Dependencies:** those the moved code uses today, `rusqlite`, `tokio` and `serde_json`; `sha2` from step 2.6. It adds no new dependency to the workspace.
- **Users:** the server, in process, in step 2.3; from step 2.4 the auditor only. The server then stops linking the crate: it seals with its own `seal.rs` and sends records over HTTP. The admin does not link it either: its chain-head check stores (sequence number, hash) pairs and asks the auditor for the record at that number over the read API ([0025](0025-audit-record-reliability.md)).
- **Record IDs:** the server's `ids::new_id` (ULIDs) moves to `askbar-service` ([0036](0036-askbar-service-crate.md)) in step 2.3, since every service makes IDs; this crate uses it from there. The server's `cost` module stays in the server.
- **Rules:**
  - Plaintext prompts, replies and provider errors never enter the store: a payload record takes only a `Sealed` value, a key ID and ciphertext ([0030](0030-sealed-audit-content.md) rule 5). That type is defined here; only the server's `seal.rs` makes one.
  - The store's API adds and reads records; it has no call that changes or deletes one, other than retention.
- Step 2.3 changes no behaviour: the server keeps its routes and its volume, and its tests pass unchanged apart from import paths.

## Alternatives considered

- **Leaving the code in the server and copying it into the auditor:** rejected. Two copies of the schema and its migrations would drift.
- **Putting it in `askbar-service`** ([0036](0036-askbar-service-crate.md)): rejected. Every service links `askbar-service`, and only the server and the auditor need the store.
- **A `store` feature, with the sealer and the record types in the crate too:** rejected. It is one more build mechanism, and conditional code, for the same result: the server's build carries no store code either way.
- **Two crates, one for sealing and one for the store:** rejected. The sealer has one user, the server, so a crate of its own buys nothing.
- **The hash chain in the auditor only:** rejected. The prune record and retention live in the store, and the chain must cover both in one transaction.
- **Moving `cost` into the crate:** rejected. Prices are the admin's settings, and the auditor holds none ([0051](0051-auditor.md)).

## Consequences

The store's schema and migrations have one home, and the auditor and the server cannot disagree on it. The crate's own tests are the store's tests, moved with it; the sealer's tests stay in the server.

After step 2.4 the server does not link the crate, so its build carries no store code.

## Amendments

None yet.
