# 0018. Releases and migrations

Status: Accepted 
Date: 2026-11-04
Plan decisions: 19, 32

## Context

Pre-1.0 Askbar is for the owner's use only. After 1.0, deployers need a stable path that always works.

## Decision

Nothing needs migrating before 1.0. After 1.0, `main` is a stable version that always works, and changes are tested in CI and by a person in staging.

- **Branch protection:** require pull requests and passing CI on `main`, including `check-isolation.sh`.
- **Tagged releases:** deployers install version tags, not `main`.
- **Registry:** publish images to a GitHub Container Registry
- **Staging:** run a second Compose project with test data and the existing mock model. A person tests each release there before tagging.
- **Migrations:** use forward-only migrations with an automatic backup first. Roll back by restoring that backup into the previous version.
- **API:** keep `/api/client/v1` additive after 1.0.

## Alternatives considered

Not recorded.

## Consequences

An untested merge can't reach a tagged deployment. Published images don't exist until the v1 release, so pre-1.0 use builds locally.
