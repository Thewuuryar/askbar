# 0001. Record architecture decisions

Status: Accepted
Date: 2026-11-03

## Context

Decisions so far live in three places: [architecture.md](../architecture.md) (the target and the reasons for each boundary), `docs/refactor-log.md` (history of one refactor) and `docs/plan-*.md` (plans, with design decisions inside). A contributor or an agent cannot tell which choices are settled and which are open.

## Decision

Decisions that cross a boundary live as ADRs in `docs/adr/`, numbered, short, and not rewritten once accepted. `architecture.md` stays the description of the target and links to ADRs for the reasons. `refactor-log.md` and the `plan-*` and `handoff-*` files stay as history.

## Alternatives considered

Not recorded.

## Consequences

Agents and contributors check `docs/adr/` before reopening a choice, and propose an ADR instead of deciding alone ([AGENTS.md](../../AGENTS.md)). `scripts/check-docs.sh` checks the links in these files. Nothing checks that a boundary change comes with an ADR; reviewers do, with the PR template.
