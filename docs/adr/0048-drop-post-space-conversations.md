# 0048. Drop `POST /spaces/{space_id}/conversations`

Status: Accepted
Date: 2026-11-08
Amends: [0047](0047-generation-routes-under-spaces.md), the space route list
Source: owner decision after review of 0047

## Context

[0022](0022-api-path-conventions.md) listed `POST /api/client/v1/spaces/{space_id}/conversations`, and [0047](0047-generation-routes-under-spaces.md) kept the line with the note "Not built yet". The code has never registered it: `POST /spaces/{space_id}/chat` creates a conversation. The owner asked to drop the route. 0047 is Accepted and on `main`, so under [0001](0001-decision-records-and-change-rules.md) rule 2 and [0046](0046-component-and-register-adrs-and-partial-amendments.md) the change is a new ADR.

## Decision

The space route list in 0047 loses its second line. The list is now:

- `GET /api/client/v1/spaces`
- `GET /api/client/v1/spaces/{space_id}/conversations`
- `GET /api/client/v1/spaces/{space_id}/conversations/{id}`
- `DELETE /api/client/v1/spaces/{space_id}/conversations/{id}`
- `GET /api/client/v1/spaces/{space_id}/search`
- `POST /api/client/v1/spaces/{space_id}/chat`
- `GET /api/client/v1/spaces/{space_id}/generations`
- `GET /api/client/v1/spaces/{space_id}/generations/{id}/stream`
- `POST /api/client/v1/spaces/{space_id}/generations/{id}/cancel`

That is the nine routes the code registers. A conversation is created by its first `chat` call. Everything else in 0047 and 0022 stands.

## Alternatives considered

- **Build the route:** rejected by the owner. It would duplicate what `chat` does.
- **Edit 0047 in place:** rejected. It is not an editorial edit under [0035](0035-editorial-edits-to-accepted-adrs.md), and 0047 is a decision ADR, not a component ADR.

## Consequences

- The list matches the code and `docs/openapi/client.v1.yaml`, which the server OpenAPI test enforces against the code.
- After 1.0 no client may rely on this route, and adding it later is an additive change.

## Amendments
