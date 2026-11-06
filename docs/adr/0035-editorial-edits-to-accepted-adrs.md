# 0035. Editorial edits to accepted ADRs

Status: Proposed
Date: 2026-11-07
Source: owner review of the v1 layout change

## Context

Under [0001](0001-decision-records-and-change-rules.md) rule 2, once an ADR is on `main` only its status line changes, and a changed decision needs a new ADR that supersedes it. Files and terms still move: the layout in [0033](0033-repository-layout.md) moved `askbar-proto/src/problem.rs`, which [0023](0023-api-error-format.md) names as the error registry, to `crates/askbar-proto/src/problem.rs`. Left alone, accepted ADRs would point at paths that no longer exist, and a later reader couldn't follow them.

## Decision

- An accepted ADR may get an **editorial edit**: one that changes no decision, rule, scope or consequence. Editorial edits are limited to a moved file path, a broken or moved link, a typo, and a term renamed through the [glossary](../architecture.md#glossary).
- Each editorial edit goes in its own commit, `docs(adr): editorial: <what>`, and the pull request lists it.
- Anything else still needs a new ADR that supersedes the old one, under 0001 rule 2. This ADR adds one exception to that rule; 0001's other rules stand, and 0001 is not superseded.

## Alternatives considered

- **Strictly immutable ADRs:** rejected. Paths and names go stale, and the current location of a fact has to be found elsewhere.
- **Editing 0001 in place:** rejected. It would break 0001's own rule and lose the record of when the rule changed.

## Consequences

ADRs stay usable as the code moves. A reviewer checks that an editorial commit changes no meaning; git history keeps the original wording. Not enforced by a script. The first editorial edit is the 0023 path change, which came in with the layout commit before this rule existed rather than in its own commit.
