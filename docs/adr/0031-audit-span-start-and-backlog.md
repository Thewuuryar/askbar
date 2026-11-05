# 0031. Audit span start and backlog

Status: Proposed
Date: 2026-11-04
Source: required by v1 decisions

## Context

Fail-closed auditing only works if a model call can't begin before its record exists.

## Decision

1. **Span start:** the server gets the auditor's acknowledgment of a span's start before asking `model-gateway` to call a provider.
2. **`model-gateway` check:** it refuses a call without a valid audit span, and records the refusal.
3. **Refusal:** if the auditor is unreachable or behind, new replies are refused with `503 audit_backlog` and a retry hint.
4. **Later records:** finish and payload records follow [0025](0025-audit-record-reliability.md).

## Alternatives considered

- **Writing records asynchronously:** rejected, because a reply could start before its record exists, and a failed write could lose the record.

## Consequences

Each reply adds an auditor round trip, which counts toward the 200 ms latency target. If the auditor is down, model work stops until it recovers. Failure tests cover an unreachable auditor, a full disk, and a missing span.
