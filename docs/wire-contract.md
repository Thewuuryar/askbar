# Wire contract

What the desktop client and the server agree on. The types live in `askbar-proto`;
this page says what they mean. Everything is JSON over HTTPS except the chat stream.
`/api/v1` is the device API. The admin site and its `/admin/api` are not part of this
contract.

## Ids

- Every id the server hands out is a **ULID** (26 characters, Crockford base32), generated
  by the server: users, devices, conversations, messages, providers.
- Ids sort in creation order within one server process. Clients treat them as opaque
  strings and never make them up.
- Timestamps are UTC unix seconds (`created_at`, `updated_at`).
- Another user's ids behave exactly like ids that do not exist: `404`.

## Authentication and the version gate

- Every request except `GET /api/v1/health` carries
  `x-askbar-client-version: <major.minor.patch>` (a leading `v` and a `-suffix` are
  ignored).
- The gate runs before authentication:
  - header missing or not a version: `400`;
  - version older than the server's `min_client`: `426` (the client shows "update needed");
  - otherwise the request goes on.
- `min_client` is currently **0.3.0**. `GET /api/v1/health` reports it:
  `{"api_version": 1, "min_client": "0.3.0"}` and needs neither header nor token.
- After the gate, calls other than `POST /api/v1/pair` need `Authorization: Bearer <device token>`;
  a missing, unknown or revoked token is `401`.
- `GET /api/v1/whoami` answers `204` if the token is accepted.

### Pairing

1. The admin shows a pairing string: `askbar-pair:<base64url(json {"url", "code"})>`
   (no padding). The code works once and expires after 10 minutes.
2. `POST /api/v1/pair` with `{"code": "..."}` returns `{"token": "abt_..."}`. A wrong,
   expired or used code is `401`. The token is shown once; the server keeps only its hash.
3. An administrator can revoke a device; its token then gets `401`.

## Providers

`GET /api/v1/providers` returns a list of
`{"id": "<ULID>", "name": "...", "type": "openai" | "anthropic", "model": "..."}`,
ordered by name. Keys never appear.

**Providers are addressed by `id`, never by name.** `ChatRequest.provider` and the
active-provider preference hold an id; a name is not accepted as a fallback (`404`).

## Preferences and display defaults

Both follow the signed-in user, so all of a user's devices see the same values.

- `GET /api/v1/me/prefs` returns `{"active_provider_id": "<id>" | null}`. `null` means
  nothing was chosen; clients then use the first provider in the list.
- `PUT /api/v1/me/prefs` with the same shape replaces it and returns it. An id that is
  not a provider is `404` and changes nothing.
- `GET /api/v1/me/display-defaults` (read-only) returns
  `{"theme": "auto"|"light"|"dark", "width", "max_height", "history_width", "top_percent", "hide_on_blur"}`:
  the user's own defaults if the administrator set them, otherwise the server-wide ones.
  A client may override any field locally; a local override wins, and removing it returns
  the server's default.
- `GET /api/v1/settings` (server-wide display defaults) remains for older callers.

## Conversations and search

- `GET /api/v1/conversations` returns `[{"id", "title", "updated_at", "message_count"}]`,
  newest first, for the caller only.
- `GET /api/v1/conversations/{id}` returns the same fields plus
  `"messages": [{"id", "role": "user"|"assistant", "content", "created_at"}]`, oldest first.
- `DELETE /api/v1/conversations/{id}` returns `204`, or `404` if it is not the caller's.
- `GET /api/v1/search?q=<words>` returns up to 20
  `{"conversation_id", "message_id", "role", "snippet", "created_at"}`, best match first,
  over the caller's own messages only. All words must match; punctuation is plain text.
  An empty query returns `[]`; more than 200 characters is `400`.

## Chat stream

`POST /api/v1/chat` with
`{"conversation_id": "<id>" (optional), "provider": "<provider id>", "text": "..."}`.
Empty text is `400`; an unknown provider or a conversation that is not the caller's is
`404`. Otherwise the answer is
`200 text/event-stream`. Provider output is never passed through; the server re-emits
its own events. Each SSE message has `event:` naming the type and `data:` holding a JSON
object that repeats the type in an `"event"` field:

| event | data | meaning |
| --- | --- | --- |
| `meta` | `{"event":"meta","conversation_id":"<id>"}` | always first; the conversation this turn belongs to (new if none was sent) |
| `delta` | `{"event":"delta","text":"..."}` | the next piece of the reply; zero or more |
| `done` | `{"event":"done"}` | the reply is complete |
| `error` | `{"event":"error","message":"..."}` | the provider failed; the question is still saved |

(Comment lines starting with `:` are keep-alives.) The question is saved before the
stream starts. If the client disconnects, the server closes the upstream request and
keeps whatever part of the reply had arrived.

## Errors

Plain HTTP status codes with no body the client depends on: `400` bad request or missing
version header, `401` bad token, `404` not found (or not yours), `426` client too old,
(There is no setup mode: the server creates its master key on first start.)
