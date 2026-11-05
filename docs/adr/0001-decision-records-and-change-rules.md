# 0001. Decision records and change rules

Status: Accepted
Date: 2026-11-04
Plan decisions: 21

## Context

Askbar needs one place to record decisions that a later contributor, person or AI agent, could otherwise reopen. The owner approves decisions.

## Decision

These rules go in `AGENTS.md` and the ADR README:

1. **Approval:** only the owner can move an ADR from Proposed to Accepted.
2. **Changing a decision:** write a new ADR that supersedes the old one, and change only the old ADR's status line.
3. **Proposed ADRs:** these can be edited freely until accepted.
4. **Reversals:** a reversal must state what changed since the original decision.
5. **When an ADR is required:** any change to a boundary, container, network, volume, API audience, or hard rule, and any new dependency that is an image or a service or is outside the license allowlist ([0026](0026-dependency-license-check.md)). A library inside the allowlist needs a reason in the pull request instead.
6. **Rule changes:** a change to a security rule includes its docs and isolation tests in the same pull request.
7. **AI agents:** they can draft ADRs and open pull requests. They can't accept ADRs, merge, or change workflows without the owner's approval. Keep this rule through 1.0, then review it.
8. **Emergencies:** a security fix can ship first, with its ADR following within 7 days.

## Alternatives considered

Not recorded.

## Consequences

Accepted decisions keep their history. Agents can prepare changes, but the owner stays responsible for approvals, merges, and workflow changes through 1.0.
