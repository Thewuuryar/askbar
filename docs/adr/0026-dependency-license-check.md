# 0026. Dependency license check

Status: Accepted 
Date: 2026-11-04
Source: recommendation accepted after the clean copy

## Context

Askbar is licensed under `AGPL-3.0-or-later`. Code built into Askbar must have a license that's compatible with it.

## Decision

- CI uses a license allowlist in `cargo deny` for Rust crates.
- CI uses a similar license check for npm packages.
- A dependency outside the allowlist needs review before it's added.
- The docs state that anyone who modifies Askbar and offers it over a network must follow the AGPL's source offer requirement.

## Alternatives considered

- **Reviewing licenses by hand:** rejected, because it's easy to miss transitive dependencies.

## Consequences

An incompatible dependency fails CI before it's merged. The check doesn't replace legal advice, and third-party images still need their own license notices when published.
