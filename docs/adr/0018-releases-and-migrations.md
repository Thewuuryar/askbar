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
- **Registry:** publish images to a GitHub Container Registry (GHCR) with a release workflow, which also publishes a compose file pinned by image digest and an SBOM. Signing comes after 1.0.
- **1.0 volumes:** at 1.0 the declarations of `askbar-secrets`, `audit-data` and `control-token` are removed from Compose, and the guide says how to delete those volumes. No data volume is renamed.
- **Staging:** run a second Compose project (a staging project of the release compose file) with test data and the existing mock model. A person tests each release there before tagging.
- **Migrations:** use forward-only migrations with an automatic backup first. Roll back by restoring that backup into the previous version.
- **API:** keep `/api/client/v1` additive after 1.0; `MIN_CLIENT` does not change for 1.0.
- **Repository settings:** the owner turns on branch protection (pull requests and passing CI on `main`).

## Alternatives considered

Not recorded.

## Consequences

An untested merge can't reach a tagged deployment. Published images don't exist until the v1 release, so pre-1.0 use builds locally.

## Amendments
- 2026-11-11: release workflow and the 1.0 volumes (#70, D33 and D34, decision changes; D30): a release workflow with GHCR images, a compose file pinned by digest, an SBOM and a staging project; signing after 1.0; `MIN_CLIENT` unchanged; the three retired volume declarations are removed at 1.0; the owner turns on branch protection.
