# 0047. Generation routes under spaces

Status: Accepted; amended by 0048
Date: 2026-11-08
Amends: [0022](0022-api-path-conventions.md), the space route list
Source: plan step 1.11, as built

## Context

Step 1.11 moved `DELETE /conversations/{id}`, `GET /generations`, `GET /generations/{id}/stream` and `POST /generations/{id}/cancel` under `/api/client/v1/spaces/{space_id}`. Generations hold the prompt and the reply, so they live in the space file, and the server can open only that space's store once the route names it. The "Spaces" list in [0022](0022-api-path-conventions.md) does not name these four routes, so it no longer matches the code.

## Decision

The "Spaces" list in 0022 is replaced by this one. Space-scoped client routes include the space, so the server can check membership and open only that store:

- `GET /api/client/v1/spaces`
- `GET /api/client/v1/spaces/{space_id}/conversations`
- `POST /api/client/v1/spaces/{space_id}/conversations`. Not built yet; `POST /spaces/{space_id}/chat` creates a conversation.
- `GET /api/client/v1/spaces/{space_id}/conversations/{id}`
- `DELETE /api/client/v1/spaces/{space_id}/conversations/{id}`
- `GET /api/client/v1/spaces/{space_id}/search`
- `POST /api/client/v1/spaces/{space_id}/chat`
- `GET /api/client/v1/spaces/{space_id}/generations`
- `GET /api/client/v1/spaces/{space_id}/generations/{id}/stream`
- `POST /api/client/v1/spaces/{space_id}/generations/{id}/cancel`

The code registers nine routes (`services/server/src/lib.rs`). The list above has ten lines: the `POST .../conversations` line is the one 0022 names and the code has never registered. The owner decides whether to keep or drop it.

Everything else in 0022 stands.

## Alternatives considered

- **Leave the four routes unlisted:** rejected. The list is the contract, and an unlisted route has no rule saying it must name its space.
- **Supersede 0022:** rejected. Only one list changes, and [0046](0046-component-and-register-adrs-and-partial-amendments.md) allows a partial amendment.
- **Keep generations at the top level and look up the space from the generation ID:** rejected. The server would have to open a store before it can check membership.

## Consequences

- The space route list matches the code. The OpenAPI test (`services/server/tests/openapi.rs`) fails if a registered route and `docs/openapi/client.v1.yaml` disagree, which keeps the code and the OpenAPI file in step; no test compares this list to either.
- 0022 shows `Accepted; amended by 0047` once the owner accepts this ADR.
- If the owner drops `POST .../conversations`, a follow-up edit to this list removes that line.

## Amendments
