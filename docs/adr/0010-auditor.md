# 0010. Auditor

Status: Accepted
Date: 2026-11-04
Plan decisions: 10, 11

## Context

Audit records must survive a compromised server or admin, and their credentials must have narrow permissions. Fail-closed auditing only works if a model call can't begin before its record exists. A crash or a brief outage can leave spans unfinished, and local tampering must be noticed.

## Decision

**Credentials**

| Credential | Allows | Set by | Held by |
| --- | --- | --- | --- |
| Service append token | Add records only; no read, change, or delete | Bootstrap | Each sending service |
| Admin append token | Add admin action records only; no read, change, or delete | Bootstrap | Admin |
| Admin read token | Read logs and set retention, never below 7 days | Bootstrap | Admin |
| Audit passphrase | Unseal content | The owner, at first run | Only the owner |

- A normal admin config push can't replace these tokens. Rotation is a host command that re-runs `bootstrap` ([0005](0005-no-controller-and-self-restart.md)).

**What is recorded**

- **Admin actions:** settings (including providers, models, budgets, users, clients and pairing keys), restarts, configuration pushes, secret, retention and backup key changes, backups and restores, provider tests, admin resets, and sealed-content sessions ([0011](0011-sealed-content-access.md)). The admin refuses an action if its record can't be appended.
- **Admin records are claims.** For a service the project builds, an admin action with no matching record from the service that applied it ([0028](0028-service-boundaries-and-config-delivery.md)) raises a mismatch alert. Caddy and `cpa` write no such record, so a change to them raises none. Mismatch alerts can be deferred past v1 ([0002](0002-v1-scope-limits-and-ship-criteria.md), [0016](0016-risk-register.md)).
- **The store:** the auditor accepts appends only, and chains records by hash. The one exception is a restore into an empty store ([0013](0013-backups.md), [0051](0051-auditor.md)). Its disk early warning is below 1 GiB free (re-armed above 1.25 GiB) and its critical alert below 256 MiB, which is also where new spans are refused. Sealed content is kept 30 days by default, never less than 7.

**Span start and backlog** (model calls)

1. **Span start:** the server sends each model call to `model-gateway` with its user, space and turn. The gateway appends the span start to the auditor and waits for the acknowledgment before calling a provider, then returns the span ID to the server. The start record is small metadata only, and one span covers a turn, provider retries included.
2. **One opener:** only `model-gateway` opens model-call spans. The server adds finish and payload records to the span ID it received, so it can't start a call without a record. Tool-call spans follow [0019](0019-post-v1-order-and-tools.md).
3. **Refusal:** if the auditor is unreachable, doesn't acknowledge within a short timeout, or is behind its backlog limit, `model-gateway` refuses the call and queues a record of the refusal. The server answers `503 audit_backlog` with a retry hint. When the auditor is down or its disk is full, model work fails closed: reserve disk space, alert early, and recover from the host.
4. **Latency:** the acknowledgment uses a persistent connection, and the auditor commits in groups. CI measures it alone and fails if its 95th percentile is 20 ms or more (in two measurements in a row), inside the 200 ms budget ([0002](0002-v1-scope-limits-and-ship-criteria.md)).

**Record reliability** (for the services the project builds that append to the auditor, [0003](0003-v1-service-layout.md); Caddy's access logs are outside these rules)

5. **Finish records:** a service queues finish and payload records, and retries them until the auditor acknowledges them.
6. **Boot IDs:** a service announces a new boot ID when it connects. The auditor ends that service's earlier unfinished spans as interrupted.
7. **Chain-head check:** the admin stores the latest chain head at intervals and checks it against the auditor. A rollback, deletion or edit appears in the admin and sends a `chain_mismatch` alert (once per finding). After a restore of the auditor's store the admin re-anchors its stored head, so the restore is not read as a rollback ([0013](0013-backups.md)). The admin-claim mismatch alerts above stay deferred (phase 8); they are not `chain_mismatch`.

## Alternatives considered

- **Writing records asynchronously:** rejected. A reply could start before its record exists, and a failed write could lose it.
- **Starting the call before the acknowledgment, with a local write-ahead log:** rejected. A compromised server could skip the record, and it breaks fail-closed. A bounded buffer for short auditor outages can be revisited if refusals annoy users.
- **The server opening spans, with a token signed by the auditor for `model-gateway` to check:** rejected. It needs a key pair to distribute and rotate, and opening spans in the gateway gives the same latency and guarantee.
- **Dropping unfinished records:** rejected. It would leave gaps in investigations.
- **An off-host copy of the chain head in v1:** rejected to keep v1 smaller ([0006](0006-v1-outbound-access-and-alerts.md)). Without it, only the admin's check notices tampering.

## Consequences

- If the auditor is down, model work stops until it recovers. Each reply adds an auditor round trip, which counts toward the 200 ms latency target.
- Payload records follow the reply start, so a crash mid-stream can leave a reply without content, marked interrupted. Services need a bounded retry queue.
- Records sent after a server or admin compromise can be false or missing, so [0016](0016-risk-register.md) adds independent records and alerts. A compromised `model-gateway` can skip a span start, since it can already reach providers directly in v1; its independent records and those alerts cover that.
- A fully compromised admin and auditor, or host root, could still agree on false data. This risk is accepted for v1 ([0016](0016-risk-register.md)).
- Failure tests cover an unreachable auditor, a full disk, and a span start that isn't acknowledged.

## Amendments

- 2026-11-10: merged in 0031 (audit span start and backlog), as "Span start and backlog", rules 1 to 4, and 0025 (audit record reliability), as "Record reliability", rules 5 to 7, with their alternatives and consequences. The rule on model-call spans, which 0010 had restated from 0031, is now said once (rule 2).
- 2026-11-11: disk alert thresholds and the re-anchor after a restore (#70, D6 and D25): the early warning and critical alert, and the restore into an empty store as the one exception to append-only. The admin-claim mismatch alerts stay deferred.
- 2026-11-12: the acknowledgment's 20 ms is a gate (plan step 7.2, PR AS, #85, D31). `services/auditor/tests/latency.rs` used to print the figures and fail only above the 1 s a caller waits; it now fails if the 95th percentile is 20 ms or more, for one caller and for four at once, and measures again at once so that only two runs over the limit fail. On a development machine it is 2 to 3 ms. The gate changed; the target did not.
- 2026-11-12: built the restore exception and the re-anchor (plan step 6.3, PR AN; [0013](0013-backups.md), [0051](0051-auditor.md)). Built so: `POST /restore` on the `admin-auditor-restore` link opens the staged store as a copy, runs `chain::verify` on all of it, and only then has the writer swap it in, in one step, between batches of records. "Empty" means no audited model call (no span, no payload): the hellos, the admin's own action records, Caddy's access lines and alert deliveries a start leaves are the install setting itself up, and the restore replaces them, so the new install's own earlier records (a password change, the restore's request) do not survive it. After the swap the admin keeps the restored chain's head as the one it expects (`audit_chain_head`), in the live database and in the set it swaps in at its restart, so rule 7's check finds the chain as it was left and raises neither a rollback nor a rewrite; a test shows a head from before is still a rewrite. No other route changes or deletes a record.
