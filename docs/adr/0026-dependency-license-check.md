# 0026. Dependency license check

Status: Merged into 0017
Date: 2026-11-04
Source: recommendation accepted after the clean copy
Merged: 2026-11-10, into [0017](0017-dependencies-and-patches.md), "Licenses"

Its license allowlist and checks are now part of 0017. The text as accepted is in git history.

## Amendments

- 2026-11-25 (proposed, not yet accepted by the owner): the image scan is `scripts/scan-third-party-images.sh`. It runs CI's Trivy flags (the `images` job in `.github/workflows/ci.yml`, with `.trivyignore.yaml`) on every externally pinned digest in the `Dockerfile`, the bases and `searxng`, and needs `trivy` and network; `npm test` does not run it. The gap found: the `images` job's scan loop in `ci.yml` and `images.yml` do not name `searxng` or `mcp-gateway`, so their images are not scanned in CI yet. Adding them is the owner's change. `ring` is now an optional direct dependency of `askbar-service` (feature `approval`, `crates/askbar-service/Cargo.toml`); it is already in `Cargo.lock` through rustls, so no new crate is added.
