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
- **Registry:** publish images to a GitHub Container Registry (GHCR) with a release workflow (`.github/workflows/images.yml`), which also publishes a compose file pinned by image digest and an SBOM. Every image is signed by digest with keyless cosign from 1.0 (the workflow run's identity; no key is stored), and each release file is signed with it.
- **Release channels:** one trunk. Each push to `main` whose `ci` run passed publishes the images as an **edge** channel (`:main`, `:sha-<7>`). A tag `vX.Y.Z-rc.N` publishes `:X.Y.Z-rc.N` through the `staging` GitHub environment, with no reviewer. A tag `vX.Y.Z` publishes `:X.Y.Z`, `:X.Y`, `:X` and `:latest` through the `production` environment, which the owner approves. Only the owner can create a release tag. A tag makes a draft GitHub release; the owner publishes it by hand.
- **1.0 volumes:** at 1.0 the declarations of `askbar-secrets`, `audit-data` and `control-token` are removed from Compose, and the guide says how to delete those volumes. No data volume is renamed.
- **Staging:** run a second Compose project (a staging project of the release compose file) with test data and the existing mock model. A person tests each release there before tagging.
- **Migrations:** use forward-only migrations with an automatic backup first. Roll back by restoring that backup into the previous version.
- **API:** keep `/api/client/v1` additive after 1.0; `MIN_CLIENT` does not change for 1.0.
- **Repository settings:** the owner turns on a ruleset for `main` (pull requests, passing CI, linear history, no force push) and one for release tags (only the owner creates, moves or deletes them), and makes the two environments. CONTRIBUTING.md lists each setting.

## Alternatives considered

Not recorded.

## Consequences

An untested merge can't reach a tagged deployment. Images from `main` are published, so a deployer can pull instead of building; only a tag makes a signed release.

## Amendments
- 2026-11-11: release workflow and the 1.0 volumes (#70, D33 and D34, decision changes; D30): a release workflow with GHCR images, a compose file pinned by digest, an SBOM and a staging project; signing after 1.0; `MIN_CLIENT` unchanged; the three retired volume declarations are removed at 1.0; the owner turns on branch protection.
- 2026-11-12: signing from 1.0, the edge channel and the environments (PR AW1, #TBD: the number is added when the PR is opened; the owner's decisions of 2026-11-12; **decision changes**): keyless cosign signing starts with 1.0 instead of after it; images are published from `main` as an edge channel, from an rc tag to the `staging` environment and from a release tag to the `production` environment with the owner's approval; rulesets for `main` and for release tags; the consequence that published images do not exist until the release is removed. Not changed: the status of this ADR.
