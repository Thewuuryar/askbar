# 0052. The `askbar-audit` crate

Status: Proposed
Date: 2026-11-08
Kind: component
Source: plan step 2.3 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

The audit store's code is in the server today: `services/server/src/audit.rs` (the `audit.db` schema and migrations, the writer, the queries and retention) and `services/server/src/seal.rs` (sealing to the audit public key). Plan step 2.3 moves both into a library, unchanged, so that the `auditor` ([0051](0051-auditor.md)) can run the store in step 2.4. This record describes that crate, and is proposed before step 2.3 so the owner accepts it first. It follows [0010](0010-auditor.md), [0025](0025-audit-record-reliability.md), [0030](0030-sealed-audit-content.md) and [0033](0033-repository-layout.md).

## Decision

- **What it is:** `crates/askbar-audit`, a library crate in the root workspace ([0033](0033-repository-layout.md)), created in the pull request of plan step 2.3. It is not a container and opens no port.
- **What it holds** (moved as it is from the server):
  - the `audit.db` store: its own append-only migrations and `PRAGMA user_version`, separate from `control.db` and the space files; never edit an earlier migration (AGENTS.md);
  - the span types (start, finish, payload) and the writer, a bounded queue in front of one SQLite connection, with its health and dropped-record count;
  - the queries the admin's audit pages use: traces, spans, sealed payloads and statistics;
  - retention, with the 7-day floor (`MIN_RETENTION_DAYS`, [0010](0010-auditor.md));
  - `seal`: sealing a payload to the audit public key, with its key ID. It seals and nothing more: the crate has no function that opens a sealed payload, and never holds the passphrase or the private key ([0030](0030-sealed-audit-content.md)). Opening stays in the admin.
- **Dependencies:** those the moved code uses today: `rusqlite`, `tokio`, `serde_json`, `crypto_box` (the `seal` feature only) and `sha2`. It adds no new dependency.
- **Users:** the server, in process, from step 2.3; the auditor from step 2.4. Which parts the server still links after 2.4 is open question 1.
- **Rules:**
  - Plaintext prompts, replies and provider errors never enter the store: a payload record takes only a sealed value ([0030](0030-sealed-audit-content.md) rule 5).
  - The store's API adds and reads records; it has no call that changes or deletes one, other than retention.
- Step 2.3 changes no behaviour: the server keeps its routes, its volume and its tests pass unchanged.

## Alternatives considered

- **Leaving the code in the server and copying it into the auditor:** rejected. Two copies of the schema and its migrations would drift.
- **Putting it in `askbar-service`** ([0036](0036-askbar-service-crate.md)): rejected. Every service links `askbar-service`, and only the server and the auditor need the store.

## Consequences

The store's schema and migrations have one home, and the auditor and the server cannot disagree on it. The crate's own tests are the store's and the sealer's tests, moved with them.

**Open questions for the owner.** The sources do not settle these, so this record decides none of them; plan step 2.3 waits for the answers.

1. **What the server links after step 2.4.** The server still seals payloads and builds span records, but no longer writes `audit.db`. Does it link the whole crate, or does the crate split (a `seal` and record-types part, and a store part behind a feature) so that the server's build carries no store code?
2. **Two server modules the code calls.** `audit.rs` uses the server's `ids::new_id` (record IDs) and `cost` (prices for the statistics). Do they move into this crate, into `askbar-service`, or does the server pass them in?
3. **The hash chain** (plan step 2.6, [0025](0025-audit-record-reliability.md)). Does it live in this crate, so that the admin's chain-head check can reuse the same hash code, or in the auditor only?

## Amendments

None yet.
