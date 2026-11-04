# 0006. One error shape with stable codes

Status: Accepted
Date: 2026-11-03 (backfilled)

## Context

Callers (the desktop app, scripts, a future iPhone app) must react to failures without parsing prose.

## Decision

Every failure from every API, and Caddy's 404, is `application/problem+json` (RFC 9457) with `type` (`urn:askbar:problem:<code>`), `title`, `status`, `detail` (a sentence for a person) and `error` (the stable code). The registry is the `ErrorCode` enum in `askbar-proto/src/problem.rs`. Codes are only added, never renamed or removed, within `v1`. Clients branch on `error`.

## Alternatives considered

Not recorded.

## Consequences

`askbar-*/tests/openapi.rs` fail if the OpenAPI files and the registry drift. A new failure mode means a new code, a status, a title and an OpenAPI entry in the same commit. A second client can be written against the codes alone.
