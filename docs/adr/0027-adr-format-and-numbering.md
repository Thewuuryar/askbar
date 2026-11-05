# 0027. ADR format and numbering

Status: Proposed
Date: 2026-11-04
Source: recommendation accepted after the clean copy

## Context

ADRs stay useful only if they're consistent, short, and easy to find.

## Decision

- File names use `NNNN-short-title.md`, numbered in order, and numbers are never reused.
- Status is Proposed, Accepted, Superseded by NNNN, or Rejected.
- Each ADR has Context, Decision, Alternatives considered, and Consequences sections.
- An invariant is written so a test or script can check it, and Consequences say how it's enforced.
- Keep ADRs short, and link to other docs instead of repeating them.
- Write "Not recorded" when the reasoning is unknown. Don't invent it.
- The ADR README keeps an index of every ADR and its status.

## Alternatives considered

Not recorded.

## Consequences

Contributors and agents can check a decision quickly. Reviewers enforce the format. The change rules are in [0001](0001-decision-records-and-change-rules.md).
