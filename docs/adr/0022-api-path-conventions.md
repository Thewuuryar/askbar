# 0022. API path conventions

Status: Proposed
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
| Gateway | `/api/gateway/v1` | Model gateway | `server` and the judge | Internal |
| Alerts | `/api/alerts/v1` | Alert sender | `auditor` and `admin` | Internal |
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

Space-scoped client routes include the space, so the server can check membership and open only that store:

- `GET /api/client/v1/spaces`
- `GET` and `POST /api/client/v1/spaces/{space_id}/conversations`
- `GET /api/client/v1/spaces/{space_id}/conversations/{id}`
- `GET /api/client/v1/spaces/{space_id}/search`
- `POST /api/client/v1/spaces/{space_id}/chat`

**Common internal routes**

Each internal service uses the same routes where they apply: `GET /health`, `GET /status`, `PUT /config`, `POST /restart`, and `POST /exports`.

**Other new routes**

- `POST /api/client/v1/me/pairing-keys`
- `POST /api/admin/v1/users/{id}/pairing-keys`
- `POST /api/admin/v1/services/{name}/restart`

**Versions and cleanup**

- Each API has its own version. Before 1.0, paths can change to fit these rules. After 1.0, `/api/client/v1` changes stay additive.
- Before 1.0, move `/admin/api/v1` to `/api/admin/v1`, replace `/api/server/v1/models/list` with `/api/server/v1/models`, and replace `/services/{name}/{action}` with literal actions.
- `/api/client/v1/chat` becomes the space-scoped route above.

## Alternatives considered

- **`/admin/api/v1`:** rejected, because it makes the admin an exception to the pattern.
- **`/{name}/api/v1`, such as `/client/api/v1`:** consistent, but it adds more top-level paths and no shared `/api` root.
- **Flat routes with a space ID in the query or body:** simpler paths, but easier to miss a space check, and inconsistent with one store per space.
- **Version only in a header:** rejected, because a path version is easier to route and inspect.

## Consequences

Caddy routes only `/admin`, `/api/admin`, and `/api/client`, and answers 404 for other paths. Moving the admin API changes its routes, cookie, site calls, OpenAPI file, and tests. Every route is recorded in its OpenAPI file, and OpenAPI tests catch drift.
