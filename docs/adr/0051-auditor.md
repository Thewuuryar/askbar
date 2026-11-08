# 0051. The `auditor` component

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan steps 2.4 to 2.9 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07); owner review of the open questions (2026-11-08)

## Context

[0010](0010-auditor.md) decides that the audit store runs in its own container, with append-only records, narrow tokens and a hash chain. [0025](0025-audit-record-reliability.md), [0030](0030-sealed-audit-content.md) and [0031](0031-audit-span-start-and-backlog.md) decide how records are finished, sealed and started. The container does not exist yet: today `audit.db` sits inside the server, on the `audit-data` volume, and the admin reads it through `/api/server/v1/audit/*` ([0039](0039-server.md)). This record describes the component plan step 2.4 builds and steps 2.5 to 2.9 extend. It is proposed before that step so the owner accepts it first. The store's code is the `askbar-audit` crate ([0052](0052-askbar-audit-crate.md)); this record covers the container around it. It links its sources instead of repeating them.

## Decision

- **What it is:** `services/auditor`, a Dockerfile target and image `askbar-auditor`. It keeps the audit store and serves `/api/audit/v1` ([0022](0022-api-path-conventions.md)). It links `askbar-audit` ([0052](0052-askbar-audit-crate.md)) and `askbar-service` ([0036](0036-askbar-service-crate.md)).
- **What it holds:**
  - `audit.db` on a new volume, `auditor-data`. Nothing needs migrating before 1.0 ([0018](0018-releases-and-migrations.md)), so the auditor starts with an empty store rather than taking over the server's;
  - the hashes of the tokens that may call it, in `auditor-token`, written by `bootstrap` ([0049](0049-bootstrap.md));
  - metadata and sealed payloads only. It never holds the audit passphrase or the private key, and has no function that opens a sealed payload ([0030](0030-sealed-audit-content.md)).
- **Its API, `/api/audit/v1`,** by token ([0010](0010-auditor.md)):
  - *service append token* (server, and later `model-gateway` and `alerter`): add records only. Only `model-gateway`'s token may open a model-call span; the server may add finish and payload records to a span it was given ([0031](0031-audit-span-start-and-backlog.md));
  - *admin append token*: add admin action records only (plan step 2.7);
  - *admin read token*: read records and metadata, and set retention (plan step 2.5).
  - No token can change or delete a record. Records leave only by retention.
  - The auditor enforces each token's limits: each token hash maps to one caller and the record kinds it may append ([0004](0004-bootstrap-and-per-link-tokens.md)), and anything else is refused with `403`. It takes a record's caller from the token, never from the request body.
  - The wire types of `/api/audit/v1` live in `askbar-proto` ([0045](0045-askbar-proto-crate.md)).
- **What it does:**
  - acknowledges each append once it is committed, over a persistent connection, committing in groups; the acknowledgment is measured alone against a 20 ms 95th percentile ([0031](0031-audit-span-start-and-backlog.md));
  - takes a boot ID from each appending service when it connects, and ends that service's earlier unfinished spans as interrupted ([0025](0025-audit-record-reliability.md));
  - chains each record to the previous one by hash, and gives the admin the chain head to store and check (plan step 2.6, [0025](0025-audit-record-reliability.md));
  - deletes records older than the retention the admin sets: 30 days by default, never below 7 ([0010](0010-auditor.md)). Each prune is itself a chained record, appended in the same transaction as the delete: the cutoff, the number of records deleted, and the hash of the last one deleted. A chain check starts from the oldest kept record and that hash, and the admin sees every prune;
  - refuses appends when it is behind its backlog limit or its disk is full, so model work fails closed ([0010](0010-auditor.md), [0031](0031-audit-span-start-and-backlog.md)). Starting defaults, tuned by the load test of plan step 7.2:

    | Setting | Default |
    | --- | --- |
    | Acknowledgment timeout at `model-gateway` | 1 s |
    | Backlog limit | 1,000 queued records, or the oldest older than 5 s |
    | Disk early warning (alerted from phase 4) | below 1 GiB free |
    | Refuse new spans | below 256 MiB free, which leaves room for the finish and payload records of spans already open |

  - serves token counts by provider and model to the admin read token (plan step 2.5). The admin applies the prices it already holds; the auditor holds no prices;
  - holds no audit key registry of its own. The admin keeps the registry, with each key's created and retired dates and what is needed to open its records, and appends each key's creation and retirement as an admin action record ([0010](0010-auditor.md)), so the registry's history is in the chain (plan step 2.7, [0030](0030-sealed-audit-content.md) rule 7);
  - later: sends alert events to `alerter` (phase 4), and makes a read-only backup export (phase 6).
- **Caddy's access logs** stay a control in [0016](0016-risk-register.md). Caddy's log writer sends no token, so how they reach the auditor is designed in plan step 2.9, and this record is amended then ([0046](0046-component-and-register-adrs-and-partial-amendments.md)).
- **Volumes:** `auditor-data` (data). Docker creates it empty and already owned by uid 10003, because the auditor image's `/audit` directory belongs to that uid; the owner is set when the image is built, not at run time. The server's old `audit-data` is neither renamed nor mounted: it stays declared and unmounted until 1.0, like `control-token`, and holds the records from before step 2.4 until the owner copies or deletes it (the guide says how, from step 2.4). `auditor-token`, its credential volume, read-only, written by `bootstrap` ([0049](0049-bootstrap.md)): the hashes of its callers' tokens, and from phase 4 its token for `alerter`.
- **Who it runs as:** its own uid, 10003 ([0003](0003-v1-service-layout.md)), from its first instruction, with no capabilities.
- **Networks:** internal only, with no outbound route ([architecture.md](../architecture.md), Outbound access). One network per caller, shared with the auditor alone: `audit-server` and `audit-admin` in step 2.4, `audit-model-gateway` in phase 3 and `audit-alerter` in phase 4 (which also carries the alert events). A caller of the auditor reaches no other service through it, and a missing row in the Links table is a blocked path. No network pins a subnet; Docker assigns them. The [guide](../guide.md#docker-networks) says how to give Docker more address pools if a host runs out.
- **Listener:** one, `/api/audit/v1`, on one port and every interface. That is safe because the auditor joins only `audit-*` networks, each made for one caller to reach this API; `check-isolation.sh` checks that its networks are exactly those.
- **Links** ([architecture.md](../architecture.md), Links): in from the server, the admin (append and read), and later `model-gateway` and `alerter`; Caddy's link is designed in step 2.9; out only to `alerter` (alert events, per-link token, phase 4).
- **Hardening:** the shared defaults of [0032](0032-container-hardening.md), with no exception. `check-isolation.sh` lists `auditor`, checks that the running process has no capabilities (`CapEff` is 0) and no route out, that each caller reaches it only on its own network, and that no unsealed content reaches its store ([0030](0030-sealed-audit-content.md)).
- **Changes elsewhere when built:** the server stops mounting `audit-data` and appends over HTTP with a bounded retry queue for finish and payload records, answering `503 audit_backlog` when the auditor cannot take them; 0039 is amended in the same pull request (plan step 2.4). Plan step 2.5 removes `/api/server/v1/audit/*`.

## Alternatives considered

- **Keeping the audit store in the server:** rejected by [0010](0010-auditor.md). A compromised server could rewrite earlier records.
- **Writing records asynchronously, or before the acknowledgment:** rejected by [0031](0031-audit-span-start-and-backlog.md).
- **An off-host copy of the chain head in v1:** rejected by [0025](0025-audit-record-reliability.md).
- **One shared audit network:** rejected. The server, `model-gateway` and `alerter` could reach each other's ports on it. Tokens would refuse their requests, but reach would no longer follow the Links table.
- **Running as the server's uid:** rejected. It needs no exception, but each service has its own uid ([0003](0003-v1-service-layout.md)).
- **Keeping `audit-data` and changing its owner at start-up:** rejected. It needs a capability exception to 0032 for a migration that [0018](0018-releases-and-migrations.md) says isn't needed before 1.0. A one-time host command instead would put a manual step in the upgrade.
- **Verifying the chain only from the oldest kept record, with no prune record:** rejected. A deletion of the oldest records would look like retention.
- **An auditor copy of the key registry:** rejected. The admin needs the registry to open records, and two copies could disagree; the admin action records already carry its history.

## Consequences

If the auditor is down or full, model work stops until it recovers ([0010](0010-auditor.md)). Every reply adds one round trip to it. A compromised server or admin can still append false records or leave some out; the hash chain shows rewrites and deletions, not lies ([0016](0016-risk-register.md)). The isolation script has one more service to check.

Each caller adds a network (the stack has about fifteen by phase 4), and every network takes one of Docker's address pools; the [guide](../guide.md#docker-networks) covers a host that runs out.

The audit trail starts again at step 2.4: records from before it stay only in the unmounted `audit-data`, outside retention, until the owner copies or deletes that volume.

## Amendments

None yet.
