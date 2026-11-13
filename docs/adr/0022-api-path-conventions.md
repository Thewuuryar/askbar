# 0022. API path conventions

Status: Accepted
Date: 2026-11-04
Source: recommendation accepted after the clean copy

## Context

Each API needs a predictable path and its own version. The admin prefix currently differs from the other APIs, some internal paths use verbs, and shared spaces add a new level.

## Decision

**Prefixes**

| API | Prefix | Served by | Called by | Exposure |
| --- | --- | --- | --- | --- |
| Client | `/api/client/v1` | `server` | Clients | Through Caddy |
| Admin | `/api/admin/v1` | `admin` | Admin site and scripts | Through Caddy |
| Server | `/api/server/v1` | `server` | `admin` | Internal |
| Audit | `/api/audit/v1` | `auditor` | Services and `admin` | Internal |
| Model gateway | `/api/model-gateway/v1` | `model-gateway` | `server`, `admin`, and the judge | Internal |
| Alerter | `/api/alerter/v1` | `alerter` | `auditor` and `admin` | Internal |
| Executor, after v1 | `/api/executor/v1` | `server` | Executor | Through `executor-link` only |

Every API uses `/api/{name}/v{major}`. The admin site stays at `/admin`, and its session cookie uses `Path=/api/admin`, so the cookie goes only to the admin API. Any admin allowlist covers both `/admin` and `/api/admin`.

**Resources**

- Use lowercase plural nouns: `/conversations`, `/providers`.
- Use kebab-case for multi-word path segments: `/pairing-keys`.
- Use snake_case for path variables: `{space_id}`.
- Nest only when a child depends on its parent, and go no deeper than one parent and child: `/spaces/{space_id}/conversations/{id}`.
- Use query parameters to filter, sort, and page collections.
- Singletons keep singular names: `/session`, `/me`, `/settings`.

**Actions**

- Prefer standard HTTP methods on resources.
- If an action doesn't map to a method, use `POST` with a literal verb as the last segment: `/generations/{id}/cancel`.
- Don't use a variable as an action, such as `/services/{name}/{action}`.

**Spaces**

Space-scoped client routes include the space, so the server can check membership and open only that store. Generations hold the prompt and the reply, so they live in the space too:

- `GET /api/client/v1/spaces`
- `GET /api/client/v1/spaces/{space_id}/conversations`
- `GET /api/client/v1/spaces/{space_id}/conversations/{id}`
- `DELETE /api/client/v1/spaces/{space_id}/conversations/{id}`
- `GET /api/client/v1/spaces/{space_id}/search`
- `POST /api/client/v1/spaces/{space_id}/chat`
- `GET /api/client/v1/spaces/{space_id}/generations`
- `GET /api/client/v1/spaces/{space_id}/generations/{id}/stream`
- `POST /api/client/v1/spaces/{space_id}/generations/{id}/cancel`

There is no `POST .../conversations`: a conversation is created by its first `chat` call. After 1.0, adding that route would be an additive change.

**Common internal routes**

Each internal service uses the same routes where they apply: `GET /health`, `GET /status`, `PUT /config`, `POST /restart`, and `POST /exports`.

**Other new routes**

- `POST /api/client/v1/me/pairing-keys`
- `POST /api/admin/v1/users/{id}/pairing-keys`
- `POST /api/admin/v1/services/{name}/restart`

**Versions and cleanup**

- Each API has its own version. Paths are fixed at 1.0: within `/api/client/v1`, add only, and never remove or rename a field, endpoint or error code.
- Done before 1.0: `/admin/api/v1` moved to `/api/admin/v1`, `/api/server/v1/models/list` was replaced with `/api/server/v1/models`, and `/services/{name}/{action}` with literal actions.
- `/api/client/v1/chat` becomes the space-scoped route above.

## Alternatives considered

- **`/admin/api/v1`:** rejected, because it makes the admin an exception to the pattern.
- **`/{name}/api/v1`, such as `/client/api/v1`:** consistent, but it adds more top-level paths and no shared `/api` root.
- **Flat routes with a space ID in the query or body:** simpler paths, but easier to miss a space check, and inconsistent with one store per space.
- **Version only in a header:** rejected, because a path version is easier to route and inspect.
- **Generations at the top level, with the space looked up from the generation ID:** rejected. The server would have to open a store before it could check membership.
- **`POST /spaces/{space_id}/conversations`:** dropped by the owner; it would duplicate what `chat` does.

## Consequences

Caddy routes only `/admin`, `/api/admin`, and `/api/client`, and answers 404 for other paths. Moving the admin API changes its routes, cookie, site calls, OpenAPI file, and tests. Every route is recorded in its OpenAPI file, and OpenAPI tests catch drift: `services/server/tests/openapi.rs` fails if a registered route and `docs/openapi/client.v1.yaml` disagree. No test compares the space list here to either.

## Amendments

- 2026-11-08: the space route list gains the four routes plan step 1.11 moved under `/spaces/{space_id}`: delete a conversation, list generations, stream one, cancel one ([0047](0047-generation-routes-under-spaces.md)).
- 2026-11-08: `POST /spaces/{space_id}/conversations` dropped, by the owner; the list is the nine routes the code registers ([0048](0048-drop-post-space-conversations.md)).
- 2026-11-10: merged in 0047 and 0048; the list above is how it stands.
