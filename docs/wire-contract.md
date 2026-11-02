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
- `min_client` is currently **0.4.0**. `GET /api/v1/health` reports it:
  `{"api_version": 1, "min_client": "0.4.0"}` and needs neither header nor token.
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
  `"messages": [{"id", "role": "user"|"assistant", "content", "created_at"}]`, oldest first,
  and, only while something is unfinished, `"pending"`: the replies still running in this
  conversation and the messages waiting behind them (see *Generations* below).
- `DELETE /api/v1/conversations/{id}` returns `204`, or `404` if it is not the caller's.
  A reply still running in it is stopped and the messages waiting in it are dropped first.
- `GET /api/v1/search?q=<words>` returns up to 20
  `{"conversation_id", "message_id", "role", "snippet", "created_at"}`, best match first,
  over the caller's own messages only. All words must match; punctuation is plain text.
  An empty query returns `[]`; more than 200 characters is `400`.

## Chat stream

`POST /api/v1/chat` with
`{"conversation_id": "<id>" (optional), "provider": "<provider id>", "text": "...", "client_request_id": "<string>" (optional)}`.
Empty text is `400`; an unknown provider or a conversation that is not the caller's is
`404`; a conversation that already has five messages waiting is `409` (see *The queue*); when
the server's audit store cannot keep up it answers `503` with `Retry-After: 5` and
`{"error":"audit_backlog"}` and starts nothing (replies already running are not affected).
Otherwise the answer is `200 text/event-stream`. Provider output is never passed through;
the server re-emits its own events. Each SSE message has `event:` naming the type and
`data:` holding a JSON object that repeats the type in an `"event"` field:

| event | data | meaning |
| --- | --- | --- |
| `meta` | `{"event":"meta","conversation_id":"<id>","generation_id":"<id>","queued":true}` | always first. The conversation this turn belongs to (new if none was sent) and the generation running it. `queued` appears only when it is true |
| `delta` | `{"event":"delta","text":"..."}` | the next piece of the reply; zero or more. Carries an SSE `id:` line, see below |
| `done` | `{"event":"done"}` | the reply is complete (also when it was stopped with cancel: the text so far is kept) |
| `error` | `{"event":"error","message":"..."}` | the reply failed or was cut short; the question is still saved, and so is any text that had arrived |

(Comment lines starting with `:` are keep-alives. A client ignores events it does not know.)

**Delta ids.** Every `delta` carries `id: <n>`, counting that generation's deltas from 1.
After a dropped connection a client resumes with `?after=<last id it saw>` and gets
everything after that, once, in order. Other events have no `id`.

**The reply does not depend on the connection.** The question is saved, and the server
generates the reply as its own task. A client that disconnects (closes the window, loses
the network) does not stop it: the upstream request stays open, the text is saved as the
assistant message when it ends, and the client can come back to it. Only cancel, deleting
the conversation, a timeout or a server stop ends it early.

**Repeating a send.** `client_request_id` is chosen by the client for each send (a random
UUID). A second `POST /api/v1/chat` from the same device with the same id starts nothing:
it attaches to the generation the first one started (same `meta`, deltas replayed from the
start) and writes no second question. The same id from another device is a different send.
A generation row is kept for 24 hours, so the guarantee holds for that long.

## Generations

A generation is one reply being produced by the server. Its status is one of
`queued`, `running`, `done`, `error`, `cancelled`, `timed_out`, `interrupted`.

- `queued`: waiting behind another reply in the same conversation. Its question is held
  with it and is **not** in `messages` yet.
- `running`: the question is saved and the provider is being read.
- `done`, `error`: the reply finished or the provider failed.
- `cancelled`: stopped with cancel. The text so far is saved as the assistant message.
- `timed_out`: stopped by the generation timeout (`generation_timeout_secs`); the text so
  far is saved.
- `interrupted`: the server restarted or stopped while it was running (the text it had is
  saved) or waiting (nothing was saved; it never ran).

Endpoints (all need a device token; another user's generation is `404`):

- `GET /api/v1/generations/{id}/stream?after=N`: the events of that generation after its
  first `N` deltas (`N` defaults to 0), then live ones, ending with `done` or `error`.
  While the generation runs, and for five minutes after it ends, this is exact (original
  `id:`s). Once only the database remembers it, delta boundaries are gone: with `after=0`
  you get the whole stored text as one `delta` (id 1), with `after>0` only the ending, and
  the client should reload the conversation to read the saved message. `cancelled` ends
  with `done`; `error`, `timed_out` and `interrupted` end with an `error` event.
- `POST /api/v1/generations/{id}/cancel`: stop it. `204`, also when it has already
  finished (nothing changes). A running generation ends as `cancelled` with its text kept;
  a queued one ends as `cancelled` and nothing is written to the conversation.
- `GET /api/v1/generations?status=running`: the caller's running **and** queued
  generations, oldest first, as
  `[{"generation_id", "conversation_id", "status", "text", "prompt", "client_request_id"?}]`.
  `text` is what the server last saved (it can trail the live stream by about a second); to
  show the reply, attach with the stream endpoint instead. Any other `status` value is `400`.
  `ConversationDetail.pending` holds the same objects for one conversation.

### The queue

A send to a conversation that already has a running or queued generation is queued, not
refused: the answer is `200` and `meta` carries `"queued": true`; the connection stays open
on keep-alives until the message starts and then streams its reply like any other. When a
generation ends, in any status, the oldest queued one in that conversation starts: its
question is stored then, its history is built then (so it sees the reply before it), and the
stored order is question, reply, question, reply. Up to **5** messages may wait per
conversation; the sixth gets `409`.

## Versions

The wire format changed in 0.4.0 (generation ids, delta ids, queued messages, the new
endpoints), and a 0.3.x client would stop a reply when its connection dropped, so
`min_client` moved to 0.4.0.

## Audit (management API)

The admin service reads what the server recorded through three endpoints on the management
listener. They are not part of the device API: they need the management token, the device API
has no such routes (`404`), and the admin never opens `audit.db` itself. Everything they return
is **metadata**; no prompt or reply text is ever in `audit.db` at this stage. A server with no
audit store answers `503`.

- `GET /mgmt/v1/audit/traces?status=&provider=&before=&limit=`: one summary per generation,
  newest first: `{"traces": [{"trace_id", "span_id", "status", "started_at", "ended_at", "user_id",
  "device_id", "tokens_in", "tokens_out", "tokens_cached", "error_code", "provider_id", "model",
  "ttft_ms", "latency_ms"}]}`. `status` filters on the turn's status, `provider` on the provider of
  its last attempt, `before` is a `trace_id` (only older traces, for paging), `limit` is 1 to 200
  (default 50). `trace_id` is the generation id.
- `GET /mgmt/v1/audit/traces/{id}`: `{"trace_id", "spans": [...]}`, every span of that generation
  in the order they began (`404` if there is none). A span has `id`, `parent_id`, `kind`, `name`,
  `status`, `started_at`, `ended_at`, `provider_id`, `model`, `tokens_in|out|cached`, `ttft_ms`,
  `error_code`, `user_id`, `device_id` and `attrs` (a JSON object).
- `GET /mgmt/v1/audit/stats?since=<unix seconds>` (default: the last 24 hours):
  `{"since", "requests", "by_status": {...}, "error_rate", "latency_ms": {"p50","p95"},
  "ttft_ms": {"p50","p95"}, "tokens": {"in","out","cached"}}` over generations that began at or
  after `since`. `error_rate` counts `error` and `timed_out`.

### audit.db

A separate SQLite file (`ASKBAR_AUDIT_DIR/audit.db`, the `audit-data` volume in the compose
setup), with its own migrations and `PRAGMA user_version`, written only by the server.

- `spans(id, trace_id, parent_id, kind, name, status, started_at, ended_at, provider_id, model,
  tokens_in, tokens_out, tokens_cached, ttft_ms, error_code, user_id, device_id, attrs_json)`.
  `kind` is `turn` (a whole generation), `gate` (a check or a wait, e.g. `queue`), `attempt` (one
  request to a provider), `tool` or `agent_step`. `trace_id` is the generation id; an attempt's
  parent is its turn. Times are unix seconds; an attempt's `attrs` carry `latency_ms`,
  `http_status`, `usage_estimated` and `finish_reason`. A span starts `running` and ends `done`,
  `error`, `cancelled`, `timed_out` or `interrupted` (the last for spans a restart cut off).
- `payloads(span_id, role, key_id, ciphertext, created_at)`: reserved for sealed content.
- The database enforces its own rules with triggers: nothing younger than 7 days is deleted, and
  a span can change only while it is `running`. Records older than `audit_retention_days` (at
  least 7) are deleted once a day.

## Errors

Plain HTTP status codes with no body the client depends on: `400` bad request or missing
version header, `401` bad token, `404` not found (or not yours), `409` the conversation
already has five messages waiting, `426` client too old,
(There is no setup mode: the server creates its master key on first start.)
