# 0051. The `auditor` component

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan steps 2.4 to 2.9 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07); owner review of the open questions (2026-11-08)

## Context

[0010](0010-auditor.md) decides the audit rules: append-only records, narrow tokens, span start, backlog, record reliability and the hash chain. [0011](0011-sealed-content-access.md) decides how content is sealed. Today `audit.db` sits inside the server, on `audit-data`, and the admin reads it through `/api/server/v1/audit/*` ([0039](0039-server.md)). This record describes the container that plan step 2.4 builds around the `askbar-audit` crate ([0052](0052-askbar-audit-crate.md)), and that steps 2.5 to 2.9 extend.

## Decision

- **What it is:** `services/auditor`, image `askbar-auditor`. It serves `/api/audit/v1` ([0022](0022-api-path-conventions.md)) and links `askbar-audit` and `askbar-service` ([0036](0036-askbar-service-crate.md)).
- **What it holds:**
  - `audit.db` on a new volume, `auditor-data`. It starts empty: nothing needs migrating before 1.0 ([0018](0018-releases-and-migrations.md));
  - its callers' token hashes, in `auditor-token`, written by `bootstrap` ([0049](0049-bootstrap.md));
  - metadata and sealed payloads only. It never holds the passphrase or the private key, and has no function that opens a payload.
- **Its API, by token** ([0010](0010-auditor.md)):
  - service append: the server, later `model-gateway` and `alerter`;
  - admin append: admin action records (plan step 2.7);
  - admin read: records, metadata and retention (plan step 2.5).
  - Each token hash maps to one caller and the record kinds it may append; anything else is `403`. The caller comes from the token, never from the body.
  - No token changes or deletes a record.
  - The wire types live in `askbar-proto` ([0045](0045-askbar-proto-crate.md)).
- **What it does beyond 0010's rules:**
  - **Prune records:** each prune is a chained record, written in the same transaction as the delete. It holds the cutoff, the count, and the hash of the last record deleted. A chain check starts from the oldest kept record and that hash, and the admin sees every prune.
  - **Token counts:** it serves them by provider and model to the admin read token. The admin applies prices; the auditor holds none.
  - **No key registry:** the admin keeps it and appends each key's creation and retirement as admin action records, so its history is in the chain ([0011](0011-sealed-content-access.md) rule 6).
  - **Later:** alert events to `alerter` (phase 4), and a read-only backup export (phase 6).
- **Starting limits,** tuned by the load test of plan step 7.2:

  | Setting | Default |
  | --- | --- |
  | Acknowledgment timeout at `model-gateway` | 1 s |
  | Backlog limit | 1,000 queued records, or the oldest older than 5 s |
  | Disk early warning (alerted from phase 4) | below 1 GiB free |
  | Refuse new spans | below 256 MiB free, leaving room for the finish and payload records of open spans |

- **Volumes:**
  - `auditor-data`: Docker creates it empty and owned by uid 10003, because the image's `/audit` belongs to that uid.
  - `auditor-token`: its credential volume, read-only.
  - The server's old `audit-data` is neither renamed nor mounted. It stays declared until 1.0 and holds the pre-2.4 records until the owner copies or deletes it (the guide says how).
- **Who it runs as:** uid 10003 ([0003](0003-v1-service-layout.md)), from its first instruction, with no capabilities.
- **Networks:** internal only, with no outbound route. There is one network per caller, shared with the auditor alone: `audit-admin` (step 2.4a, built), `audit-server` (with the server's switch to the auditor, step 3.1), `audit-model-gateway` (phase 3) and `audit-alerter` (phase 4, which also carries alert events). No subnet is pinned ([guide](../guide.md#docker-networks)).
- **Listener:** one port, every interface. That is safe because it joins only `audit-*` networks, each for one caller.
- **Links** ([architecture.md](../architecture.md), Links):
  - in: from the server, the admin (append and read), and later `model-gateway` and `alerter`;
  - out: only to `alerter` (phase 4);
  - Caddy's access logs: designed in plan step 2.9 (its log writer sends no token), and this record is amended then.
- **Hardening:** [0032](0032-container-hardening.md)'s defaults, with no exception. `check-isolation.sh` checks:
  - `CapEff` is 0;
  - there is no route out;
  - each caller reaches it only on its own network, and its networks are exactly the `audit-*` ones;
  - no unsealed content reaches its store.
- **Changes elsewhere when built:** the server stops mounting `audit-data` and appends over HTTP with a bounded retry queue, answering `503 audit_backlog` ([0010](0010-auditor.md) rule 3); 0039 is amended in that pull request. Step 2.5 removes `/api/server/v1/audit/*`.

## Alternatives considered

- **One shared audit network:** rejected. Callers could reach each other's ports; tokens would refuse them, but reach would no longer follow the Links table.
- **Running as the server's uid:** rejected. Each service has its own uid.
- **Keeping `audit-data` and changing its owner at start-up:** rejected. It needs a capability exception to 0032, for a migration 0018 says isn't needed; a host command instead adds a manual upgrade step.
- **No prune record:** rejected. Deleting the oldest records would look like retention.
- **An auditor copy of the key registry:** rejected. The admin needs it to open records, and two copies could disagree.

The decisions behind the auditor itself (its own container, synchronous acknowledgment, no off-host chain head) are in [0010](0010-auditor.md).

## Consequences

- If the auditor is down or full, model work stops ([0010](0010-auditor.md)).
- A compromised caller can still append false records or leave some out; the chain shows rewrites and deletions, not lies ([0016](0016-risk-register.md)).
- Each caller adds a network (about fifteen by phase 4), and each takes one of Docker's address pools; the [guide](../guide.md#docker-networks) covers a host that runs out.
- The audit trail restarts at step 2.4: older records stay only in the unmounted `audit-data`.

## Amendments

- 2026-11-10: trimmed: decisions that 0010 and 0011 make are linked instead of restated (token scopes, acknowledgment and latency, boot IDs, retention, backlog), and links to the merged 0025, 0030, 0031 and 0046 point to 0010, 0011 and 0001. No decision changed.
- 2026-11-10: built standing alone in plan step 2.4a (PR: feat/auditor): nothing sends it records until step 3.1, and the server still records to its own file. Built so: its links are `server-auditor`, `model-gateway-auditor`, `admin-auditor-append` and `admin-auditor-read` (only the two admin links have tokens from `bootstrap` yet, [0049](0049-bootstrap.md)); the server's link opens `gate` spans only and `model-gateway`'s any kind; a hello ends that service's earlier unfinished spans as interrupted; admin action records and prune records are tables of their own, never changed or deleted (a prune record holds the cutoff and the counts; the hash of the last record deleted comes with the chain, plan step 2.6); a backlog over 1,000 queued records or an oldest record waiting over 5 s refuses every append, and below 256 MiB free a new span, with `503 audit_backlog`; free space is read from `df`; the network `audit-admin` is built, the others come with their callers.
- 2026-11-10: `model-gateway-auditor` has its token from `bootstrap`, and the auditor joins `audit-model-gateway` (`internal`, shared with the gateway alone), so the gateway is the first caller (plan step 3.1, [0054](0054-model-gateway.md)). `server-auditor` and `audit-server` still come with the server's switch.
- 2026-11-11: the server is a caller (plan step 3.1, second half): `server-auditor` appends its gate spans, sealed payloads and the finish of the turn span the gateway opened, over the network `audit-server` (`internal`, the auditor and the server alone); a temporary `server-auditor-read` link reads traces, a trace's spans, a span's sealed payloads, stats and token counts for the Audit pages, which the server prices, and nothing else (no admin actions, prunes or retention). The stats gain `requests_with_tokens` on each group and `by_day_provider` (tokens for each day and provider, no cost), so whoever holds the prices can price each day exactly. The server's own `audit.db` is not opened any more.
- 2026-11-11: the admin reads the store (plan step 2.5): the Audit pages ask `admin-auditor-read` over `audit-admin` for traces, spans, sealed payloads and stats, and the admin applies its prices, so the auditor still holds none. The admin also sets the retention (`PUT /retention`) from its own setting, every 30 seconds and after a change. The temporary `server-auditor-read` link is removed, with its route set.
- 2026-11-11: serves the hash chain to `admin-auditor-read` (plan step 2.6, [0052](0052-askbar-audit-crate.md)): `GET /chain/head` (the newest entry's number and hash), `GET /chain/entries/{id}` (one entry's hash) and `GET /chain/verify` (a full check of every link and record, at most 20 problems listed). No other link may call them.
