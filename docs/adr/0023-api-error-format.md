# 0023. API error format

Status: Accepted 
Date: 2026-11-04
Source: recommendation accepted after the clean copy

## Context

Clients and scripts must react to failures without parsing prose.

## Decision

Every failure from every API, and Caddy's 404, is `application/problem+json` (RFC 9457). It includes:

- `type`: `urn:askbar:problem:<code>`
- `title`
- `status`
- `detail`: a sentence for a person
- `error`: the stable code clients branch on

The `ErrorCode` enum in `crates/askbar-proto/src/problem.rs` is the registry. After 1.0, codes in `v1` are only added, never renamed or removed.

## Alternatives considered

Not recorded.

## Consequences

A new failure mode needs a code, a status, a title, and an OpenAPI entry in the same commit. OpenAPI tests fail if the files and the registry drift.
