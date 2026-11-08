# 0050. The `auditor` component

Status: Proposed
Date: 2026-11-08
Kind: component
Source: plan steps 2.4 to 2.9 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

[0010](0010-auditor.md) decides that the audit store runs in its own container, with append-only records, narrow tokens and a hash chain. [0025](0025-audit-record-reliability.md), [0030](0030-sealed-audit-content.md) and [0031](0031-audit-span-start-and-backlog.md) decide how records are finished, sealed and started. The container does not exist yet: today `audit.db` sits inside the server, on the `audit-data` volume, and the admin reads it through `/api/server/v1/audit/*` ([0039](0039-server.md)). This record describes the component plan step 2.4 builds and steps 2.5 to 2.9 extend. It is proposed before that step so the owner accepts it first. The store's code is the `askbar-audit` crate ([0051](0051-askbar-audit-crate.md)); this record covers the container around it. It links its sources instead of repeating them.

## Decision

- **What it is:** `services/auditor`, a Dockerfile target and image `askbar-auditor`. It keeps the audit store and serves `/api/audit/v1` ([0022](0022-api-path-conventions.md)). It links `askbar-audit` ([0051](0051-askbar-audit-crate.md)) and `askbar-service` ([0036](0036-askbar-service-crate.md)).
- **What it holds:**
  - `audit.db` on the `audit-data` volume, which moves from the server to the auditor and keeps its name ([architecture.md](../architecture.md): data volumes are never renamed);
  - the hashes of the tokens that may call it, written by `bootstrap` ([0049](0049-bootstrap.md));
  - metadata and sealed payloads only. It never holds the audit passphrase or the private key, and has no function that opens a sealed payload ([0030](0030-sealed-audit-content.md)).
- **Its API, `/api/audit/v1`,** by token ([0010](0010-auditor.md)):
  - *service append token* (server, and later `model-gateway`, `alerter` and Caddy): add records only. Only `model-gateway`'s token may open a model-call span; the server may add finish and payload records to a span it was given ([0031](0031-audit-span-start-and-backlog.md));
  - *admin append token*: add admin action records only (plan step 2.7);
  - *admin read token*: read records and metadata, and set retention (plan step 2.5).
  - No token can change or delete a record. Records leave only by retention.
- **What it does:**
  - acknowledges each append once it is committed, over a persistent connection, committing in groups; the acknowledgment is measured alone against a 20 ms 95th percentile ([0031](0031-audit-span-start-and-backlog.md));
  - takes a boot ID from each appending service when it connects, and ends that service's earlier unfinished spans as interrupted ([0025](0025-audit-record-reliability.md));
  - chains each record to the previous one by hash, and gives the admin the chain head to store and check (plan step 2.6, [0025](0025-audit-record-reliability.md));
  - deletes records older than the retention the admin sets: 30 days by default, never below 7 ([0010](0010-auditor.md));
  - refuses appends when it is behind its backlog limit or its disk is full, so model work fails closed ([0010](0010-auditor.md), [0031](0031-audit-span-start-and-backlog.md)). It keeps a disk reserve and raises an early warning (alerted from phase 4).
  - Later: sends alert events to `alerter` (phase 4), takes Caddy's access logs (plan step 2.9), and makes a read-only backup export (phase 6).
- **Volumes:** `audit-data` (data); its credential volume with the token hashes, from `bootstrap`.
- **Networks:** internal only, with no outbound route ([architecture.md](../architecture.md), Outbound access). Each caller reaches it on a network it shares with the auditor alone, so a caller of the auditor reaches no other service through it.
- **Links** ([architecture.md](../architecture.md), Links): in from the server, the admin (append and read), and later `model-gateway`, `alerter` and Caddy; out only to `alerter` (alert events, per-link token, phase 4).
- **Hardening:** the shared defaults of [0032](0032-container-hardening.md), with no exception stated here; `check-isolation.sh` lists `auditor`, checks that it has no route out, and checks that no unsealed content reaches its store ([0030](0030-sealed-audit-content.md)).
- **Changes elsewhere when built:** the server stops mounting `audit-data` and appends over HTTP with a bounded retry queue for finish and payload records, answering `503 audit_backlog` when the auditor cannot take them; 0039 is amended in the same pull request (plan step 2.4). Plan step 2.5 removes `/api/server/v1/audit/*`.

## Alternatives considered

- **Keeping the audit store in the server:** rejected by [0010](0010-auditor.md). A compromised server could rewrite earlier records.
- **Writing records asynchronously, or before the acknowledgment:** rejected by [0031](0031-audit-span-start-and-backlog.md).
- **An off-host copy of the chain head in v1:** rejected by [0025](0025-audit-record-reliability.md).

## Consequences

If the auditor is down or full, model work stops until it recovers ([0010](0010-auditor.md)). Every reply adds one round trip to it. A compromised server or admin can still append false records or leave some out; the hash chain shows rewrites and deletions, not lies ([0016](0016-risk-register.md)). The isolation script has one more service to check.

**Open questions for the owner.** The sources do not settle these, so this record decides none of them; plan step 2.4 waits for the answers.

1. **Network names and layout.** One internal network per caller (`audit-server`, `audit-admin`, ...) or one shared audit network? A shared one lets each caller reach the others on it. New network names are part of the stack's shape.
2. **The existing `audit.db` changes owner.** Today its files belong to the server's uid (10002). Does the auditor get its own uid, with a one-time step that changes the files' owner on upgrade (which needs a capability, an exception to [0032](0032-container-hardening.md)), or does it run as the server's uid?
3. **Retention and the hash chain.** Deleting the oldest records removes the start of the chain. How does a check verify a chain whose start is gone: a checkpoint record that carries the hash of the last deleted record, or verifying from the oldest kept record only?
4. **Backlog limit, acknowledgment timeout and disk reserve.** The sources call them "short" and "reserved" without numbers.
5. **Who enforces "admin action records only".** Does the auditor accept only admin categories on the admin append token, or does it accept any record and label it as the admin's?
6. **Caddy's access logs** (plan step 2.9). Caddy's `net` log writer sends no token. An ingest listener on the auditor reachable only from Caddy's network, or a log-forwarding sidecar (a new component, with its own ADR)?
7. **The audit key registry** ([0030](0030-sealed-audit-content.md) rule 7). Today the admin keeps the key IDs; does the auditor keep the registry as audit metadata, as 0030 says, from step 2.4 or step 2.8?
8. **The server's statistics.** The admin's usage page reads token and cost totals from `audit.db` through the server today. Do the totals move to `/api/audit/v1` with the read token (step 2.5), with the prices the admin already holds?

## Amendments

None yet.
