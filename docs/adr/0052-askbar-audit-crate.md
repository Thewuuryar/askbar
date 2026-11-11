# 0052. The `askbar-audit` crate

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan step 2.3 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07); owner review of the open questions (2026-11-08)

## Context

The audit store's code lives in a library, so the `auditor` ([0051](0051-auditor.md)) can run it and no other image carries it. It follows [0010](0010-auditor.md), [0011](0011-sealed-content-access.md) and [0033](0033-repository-layout.md).

## Decision

- **What it is:** `crates/askbar-audit`, a library crate in the root workspace. No container, no port.
- **What it holds:** the store and nothing else. No Cargo features.
  - the `audit.db` store: its own append-only migrations and `PRAGMA user_version`, separate from `control.db` and the space files; never edit an earlier migration (AGENTS.md);
  - the writer: a bounded queue in front of one SQLite connection, with its health and dropped-record count;
  - the queries the admin's audit pages use: traces, spans, sealed payloads and statistics. Statistics take a callback that turns token counts into a cost, so the crate never sees a price type; the caller applies prices;
  - retention, with the 7-day floor (`MIN_RETENTION_DAYS`, [0010](0010-auditor.md));
  - the hash chain and the prune record ([0051](0051-auditor.md)), written in the same transaction as the record they cover;
  - **from plan step 4.1,** the `alert_deliveries` table (id, caller, at, kind, channel, outcome, attempts): the outcome of each alert, metadata only, with a chain entry in the same transaction;
  - **from plan step 6.2,** `Recorder::snapshot_to`, a consistent copy of the store made with `VACUUM INTO` on its own volume;
  - **from plan step 6.3,** a restore that fills an empty store from a snapshot and runs `chain::verify` first; it refuses a store that holds any record.
- **What it does not hold:**
  - sealing, in each sender's own `seal.rs` (the server and `model-gateway`). It only seals; opening stays in the admin ([0011](0011-sealed-content-access.md)).
  - the span types, which are `/api/audit/v1` wire types and live in `askbar-proto` ([0045](0045-askbar-proto-crate.md)).
- **Dependencies:** those the moved code uses today, `rusqlite`, `tokio`, `serde_json` and `sha2`. It adds no new dependency to the workspace.
- **Users:** the auditor only. The server links it only under its `test-rig` feature, and the admin and `model-gateway` only in their tests, to run a real auditor; their builds carry no store code. The admin's chain-head check stores (sequence number, hash) pairs and asks the auditor over the read API ([0010](0010-auditor.md) rule 7).
- **Record IDs:** `ids::new_id` (ULIDs) is in `askbar-service` ([0036](0036-askbar-service-crate.md)), since every service makes IDs. Prices stay with the admin.
- **Rules:**
  - Plaintext prompts, replies and provider errors never enter the store: a payload record takes only a `Sealed` value, a key ID and ciphertext ([0011](0011-sealed-content-access.md) rule 4). That type is defined here; only the server's `seal.rs` makes one.
  - The store's API adds and reads records; it has no call that changes or deletes one, other than retention.

## Alternatives considered

- **Leaving the code in the server and copying it into the auditor:** rejected. Two copies of the schema and its migrations would drift.
- **Putting it in `askbar-service`** ([0036](0036-askbar-service-crate.md)): rejected. Every service links `askbar-service`, and only the server and the auditor need the store.
- **A `store` feature, with the sealer and the record types in the crate too:** rejected. It is one more build mechanism, and conditional code, for the same result: the server's build carries no store code either way.
- **Two crates, one for sealing and one for the store:** rejected. The sealer has one user, the server, so a crate of its own buys nothing.
- **The hash chain in the auditor only:** rejected. The prune record and retention live in the store, and the chain must cover both in one transaction.
- **Moving `cost` into the crate:** rejected. Prices are the admin's settings, and the auditor holds none ([0051](0051-auditor.md)).

## Consequences

The schema and migrations have one home. The crate's tests are the store's; the sealers' stay with their senders. The server's and the gateway's builds carry no store code.

## Amendments

- 2026-11-10: trimmed, and links to the merged 0025 and 0030 point to 0010 rule 7 and 0011 rule 4. No decision changed.
- 2026-11-10: built in plan step 2.3 (#52). The server re-exports the crate as `audit` and calls `stats` with a cost callback.
- 2026-11-11: built the hash chain (plan step 2.6): migration 4 adds the `chain` table, one entry for each span start, span finish, payload, admin action and prune, written in the same transaction as the record. An entry holds a digest of its record, the hash of the entry before it and its own hash; entries are never changed or deleted, a prune included, because they hold digests and no content (so a prune writes its own entry and leaves the chain whole, instead of holding the hash of the last record it deleted). `chain::verify` checks the links, each record against its digest, records with no entry and records gone with no prune that explains them; `chain::head` and `chain::entry_hash` are what the admin's check asks for. A file from before the chain is chained when the migration runs and proves nothing about its records before that day. Adds `sha2` to the crate.
- 2026-11-11: body brought up to date with the code on `main`: the span types are in `askbar-proto`, the chain is part of the crate, and the auditor is its only user (the server links it under `test-rig`). No decision changed (#70).
- 2026-11-11: design for plan steps 4.1, 6.2 and 6.3 (#70): the chained `alert_deliveries` table, `snapshot_to` (`VACUUM INTO`), and restore into an empty store with a chain verify. The code PRs add the 'built' lines.
