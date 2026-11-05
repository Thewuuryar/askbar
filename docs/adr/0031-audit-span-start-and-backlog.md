# 0031. Audit span start and backlog

Status: Proposed
Date: 2026-11-04
Source: required by v1 decisions

## Context

Fail-closed auditing only works if a model call can't begin before its record exists.

## Decision

1. **Span start:** the server sends each model call to `model-gateway` with its user, space, and turn. The gateway appends the span start to the auditor and waits for the acknowledgment before calling a provider, then returns the span ID to the server. The start record is small metadata only, and one span covers a turn, including provider retries.
2. **One opener:** only `model-gateway` opens model-call spans. The server adds finish and payload records to the span ID it received, so it can't start a call without a record.
3. **Refusal:** if the auditor is unreachable, doesn't acknowledge within a short timeout, or is behind its backlog limit, `model-gateway` refuses the call and queues a record of the refusal ([0025](0025-audit-record-reliability.md)). The server answers `503 audit_backlog` with a retry hint.
4. **Later records:** finish and payload records follow [0025](0025-audit-record-reliability.md).
5. **Latency:** the acknowledgment uses a persistent connection, and the auditor commits in groups. CI measures it alone against a 20 ms 95th percentile target, inside the 200 ms budget ([0002](0002-v1-scope-limits-and-ship-criteria.md)).

## Alternatives considered

- **Writing records asynchronously:** rejected, because a reply could start before its record exists, and a failed write could lose the record.
- **Starting the call before the acknowledgment, with a local write-ahead log:** rejected, because a compromised server could skip the record, and it breaks fail-closed. A bounded buffer for short auditor outages can be revisited if refusals annoy users.
- **The server opening spans, with a token signed by the auditor for `model-gateway` to check:** rejected, because it needs a key pair to distribute and rotate, and opening spans in the gateway gives the same latency and the same guarantee.

## Consequences

Each reply adds an auditor round trip, which counts toward the 200 ms latency target. If the auditor is down, model work stops until it recovers. Payload records follow the reply start, so a crash mid-stream can leave a reply without content, marked interrupted ([0025](0025-audit-record-reliability.md)). A compromised `model-gateway` can skip a span start, since it can already reach providers directly in v1. Its independent records and the alerts in [0016](0016-risk-register.md) cover that. Failure tests cover an unreachable auditor, a full disk, and a missing span.
