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

## Amendments

- 2026-11-25 (accepted by the owner): research adds five problem types, each an `ErrorCode` with its status, title and detail in `askbar-proto` (`problem.rs`, `http.rs`): `task_token_cap` (402), when a research task's model calls have used its token cap; `task_tool_calls` (429), when the task has made as many tool calls as it may; `research_busy` (409), when the user already has a research report running; `research_disabled` (503), when the administrator has switched research off; and `decision_unsupported` (400), when `POST /decide` is asked for a decision the gateway does not take yet. Clients branch on these codes, never on the text.
