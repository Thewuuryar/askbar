# 0021. Names and new components

Status: Superseded by 0038
Date: 2026-11-04
Source: recommendation accepted after the clean copy

## Context

More components are planned after v1. If one role has several names, the code, APIs, and docs become harder to follow and review.

## Decision

1. Every role has one name, used in API paths, code, environment variables, UI labels, and documents.
2. The architecture glossary defines each name, and `scripts/check-names.sh` keeps retired words out.
3. A part becomes a named component when it gets its own container.
4. A new component gets a name and an ADR before it exists. Its name must not be confusable with an existing one.
5. A component with its own API audience also needs a path prefix and an OpenAPI file.

## Alternatives considered

- **Naming components as they're built:** rejected, because it lets a new service skip review and creates inconsistent names.

## Consequences

Adding a service takes a small amount of paperwork up front. The check script enforces retired words, and reviewers enforce the ADR requirement.

## Amendments
