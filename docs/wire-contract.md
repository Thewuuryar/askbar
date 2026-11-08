# Wire contract

This page describes the code on `main`. The accepted v1 target differs: see [architecture.md](architecture.md] and the [ADRs](adr/README.md). Where they disagree, build to the ADR.

What an Askbar app (a *client*) and the server agree on. The types live in `askbar-proto`;
this page says what they mean. Everything is JSON over HTTPS except the chat stream.
`/api/client/v1` is the client API. The server API (`/api/server/v1`, for the admin service
only) and the admin API (`/api/admin/v1`) are described in the sections below and in
[architecture.md](architecture.md), not here.

## The specs

The machine-readable description of each API is an OpenAPI 3.1 file; this page is the prose that
goes with them (what the pieces mean and why), and does not repeat them.

- [openapi/client.v1.yaml](openapi/client.v1.yaml): the client API, `/api/client/v1`.
- [openapi/server.v1.yaml](openapi/server.v1.yaml): the server API, `/api/server/v1` (internal).
- [openapi/admin.v1.yaml](openapi/admin.v1.yaml): the admin API, `/api/admin/v1`.

Tests check that each file lists exactly the routes the code registers.

## Ids

- Every id the server hands out is a **ULID** (26 characters, Crockford base32), generated
  by the server: users, clients, spaces, conversations, messages, providers.
- Ids sort in creation order within one server process. Clients treat them as opaque
  strings and never make them up.
- Timestamps are UTC unix seconds (`created_at`, `updated_at`).
- Another user's ids, and ids from a space the caller is not in, behave exactly like ids that do
  not exist: `404`.

## Authentication and the version gate

- Every request except `GET /api/client/v1/health` carries
  `x-askbar-client-version: <major.minor.patch>` (a leading `v` and a `-suffix` are
  ignored).
- The gate runs before authentication:
  - header missing or not a version: `400`;
  - version older than the server's `min_client`: `426` (the client shows "update needed");
  - otherwise the request goes on.
- `min_client` is currently **0.6.0**. `GET /api/client/v1/health` reports it:
  `{"api_version": 1, "min_client": "0.6.0"}` and needs neither header nor token.
- After the gate, calls other than `POST /api/client/v1/pairings` need `Authorization: Bearer <client token>`;
  a missing, unknown or revoked token is `401`.
- `GET /api/client/v1/me` answers `204` if the token is accepted.
- **The old prefix.** Everything under `/api/v1` (the first generation of this API) answers `426
  client_too_old` ("update the app") whatever the method, token or header. Nothing else of the old
  API exists.

### Pairing

1. The admin shows a pairing key: `askbar-pair:<base64url(json {"url", "code", "cert_sha256"?})>`
   (no padding). The code works once and expires after 10 minutes. `cert_sha256` (64 lowercase hex
   digits) is present while the server uses the install's own certificate instead of a publicly trusted
   one: the client then trusts exactly the certificate with that SHA-256 for this server, whatever name
   or address it connects with ([ADR 0034](adr/0034-self-signed-https-and-certificate-pinning.md)).
   A key without it is checked against the public roots as usual.
2. `POST /api/client/v1/pairings` with `{"code": "..."}` returns `{"token": "abt_..."}`. A wrong,
   expired or used code is `401`. The token is shown once; the server keeps only its hash. The new
   client belongs to the user the code was made for; a code for a user the server does not have
   pairs nothing.
3. A paired client can make a pairing key for another client of the same user ("Pair another app"):
   `POST /api/client/v1/me/pairing-keys` with `{"name": "..."}` (optional, at most 60 characters)
   returns `201 {"code": "...", "expires_in": 600}`. The client builds the key itself from its own
   server address and certificate pin, which the server does not know. At most five unused codes per
   user; a sixth is `429 too_many_attempts` with `Retry-After`. The server keeps only the code's hash.
4. An administrator can revoke a client; its token then gets `401`.

## Providers

`GET /api/client/v1/providers` returns a list of
`{"id": "<ULID>", "name": "...", "type": "openai" | "anthropic", "model": "..."}`,
the default provider first, then the rest by name. Keys never appear.

**Providers are addressed by `id`, never by name.** `ChatRequest.provider` and the
active-provider preference hold an id; a name is not accepted in place of one (`404`).

## Preferences and display defaults

Both follow the signed-in user, so all of a user's clients see the same values.

- `GET /api/client/v1/me/prefs` returns `{"active_provider_id": "<id>" | null}`. `null` means
  nothing was chosen; clients then use the first provider in the list.
- `PUT /api/client/v1/me/prefs` with the same shape replaces it and returns it. An id that is
  not a provider is `404` and changes nothing.
- `GET /api/client/v1/me/display-defaults` (read-only) returns
  `{"theme": "auto"|"light"|"dark", "width", "max_height", "history_width", "top_percent", "hide_on_blur"}`:
  the user's own defaults if the administrator set them, otherwise the server-wide ones.
  A client may override any field locally; a local override wins, and removing it returns
  the server's default.
- `GET /api/client/v1/settings` (server-wide display defaults) remains for older callers.

## Spaces

A **space** holds conversations, their messages, search and replies, in a file of its own on the
server ([ADR 0009](adr/0009-spaces-and-storage.md)). Every user has one **personal** space, made with
the user. Shared spaces come later (plan step 1.12).

- `GET /api/client/v1/spaces` returns `[{"id", "kind": "personal"|"shared", "name"}]`, the spaces the
  caller is a member of, the personal one first (its `name` is empty).
- Every route below is under `/api/client/v1/spaces/{space_id}`. The server checks that the caller's
  user is a member of that space before it opens the space; a space that does not exist and one the
  user is not in both answer `404 not_found`, so the answer says nothing about other people's spaces.
- A client uses the personal space unless the person picks another. It finds the id once with
  `GET /spaces` and keeps it for as long as it stays paired.
- Ids from one space mean nothing in another: a conversation or generation id sent under the wrong
  space is `404`.

The flat routes of 0.5.x (`/api/client/v1/conversations`, `/search`, `/chat`, `/generations`) are gone
and answer `404`; `min_client` is `0.6.0`, so a 0.5.x app is told to update first.

## Conversations and search

Paths below are relative to `/api/client/v1/spaces/{space_id}`.

- `GET /conversations` returns `[{"id", "title", "updated_at", "message_count"}]`,
  newest first, every conversation in the space.
- `GET /conversations/{id}` returns the same fields plus
  `"messages": [{"id", "role": "user"|"assistant", "content", "created_at"}]`, oldest first,
  and, only while something is unfinished, `"pending"`: the replies still running in this
  conversation and the messages waiting behind them (see *Generations* below).
- `DELETE /conversations/{id}` returns `204`, or `404` if there is no such conversation in the space.
  A reply still running in it is stopped and the messages waiting in it are dropped first.
- `GET /search?q=<words>` returns up to 20
  `{"conversation_id", "message_id", "role", "snippet", "created_at"}`, best match first,
  over the space's messages only. All words must match; punctuation is plain text.
  An empty query returns `[]`; more than 200 characters is `400`.

## Chat stream

`POST /api/client/v1/spaces/{space_id}/chat` with
`{"conversation_id": "<id>" (optional), "provider": "<provider id>", "text": "...", "client_request_id": "<string>" (optional)}`.
A new conversation is made in that space, and the history sent to the model comes from that space
only. Empty text is `400`; an unknown provider, a conversation that is not in the space, or a space
the caller is not a member of is `404`; a conversation that already has five messages waiting is `409` (see *The queue*); when
the server cannot take the message it refuses before anything starts, with a problem (see
*Errors*) whose `error` says why (the typed text is never lost; the client keeps it):

The refusals are 409 `queue_full`, 429 `budget_exceeded` (with `period`), 503 `kill_switch`,
503 `audit_key_missing` and 503 `audit_backlog` (with `Retry-After`); the OpenAPI file lists them on
`POST /spaces/{space_id}/chat`.

Each turn sends the conversation's history with the new question. A question that got no reply (its
turn failed, or was stopped, timed out or interrupted before any text) stays in the conversation but
is not sent to the model again: of several unanswered questions in a row only the newest is sent. To
retry one, send it again.

None of these affect replies already running.
Otherwise the answer is `200 text/event-stream`. Provider output is never passed through;
the server re-emits its own events. Each SSE message has `event:` naming the type and
`data:` holding a JSON object that repeats the type in an `"event"` field:

| event | data | meaning |
| --- | --- | --- |
| `meta` | `{"event":"meta","conversation_id":"<id>","generation_id":"<id>","queued":true}` | always first. The conversation this turn belongs to (new if none was sent) and the generation running it. `queued` appears only when it is true |
| `provider` | `{"event":"provider","id":"<provider id>","name":"..."}` | which provider is serving the reply: always the one asked for. Sent once, before the first delta. No SSE `id:` |
| `delta` | `{"event":"delta","text":"..."}` | the next piece of the reply; zero or more. Carries an SSE `id:` line, see below |
| `done` | `{"event":"done"}` | the reply is complete (also when it was stopped with cancel: the text so far is kept) |
| `error` | `{"event":"error","message":"...","code":"timed_out"}` | the reply failed or was cut short; the question is still saved, and so is any text that had arrived. `code` is the stable reason: `timed_out`, `interrupted`, `kill_switch`, `provider_error` or `internal` (absent from an older server). Branch on `code`; `message` is for a person |

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
UUID). A second `POST /spaces/{space_id}/chat` from the same client in the same space with the same id starts nothing:
it attaches to the generation the first one started (same `meta`, deltas replayed from the
start) and writes no second question. The same id from another client is a different send.
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

Endpoints, under `/api/client/v1/spaces/{space_id}` (all need a client token; a generation in
another space is `404`):

- `GET /generations/{id}/stream?after=N`: the events of that generation after its
  first `N` deltas (`N` defaults to 0), then live ones, ending with `done` or `error`.
  While the generation runs, and for five minutes after it ends, this is exact (original
  `id:`s). Once only the database remembers it, delta boundaries are gone: with `after=0`
  you get the whole stored text as one `delta` (id 1), with `after>0` only the ending, and
  the client should reload the conversation to read the saved message. `cancelled` ends
  with `done`; `error`, `timed_out` and `interrupted` end with an `error` event.
- `POST /generations/{id}/cancel`: stop it. `204`, also when it has already
  finished (nothing changes). A running generation ends as `cancelled` with its text kept;
  a queued one ends as `cancelled` and nothing is written to the conversation.
- `GET /generations?status=running`: the space's running **and** queued
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

The version in the path is the API version (`v1`). Within it changes are additive only: new
fields, endpoints and error codes. Anything else gets a `v2` beside it for one release.

`min_client` is a separate, smaller thing: the oldest app this server will talk to within `v1`.
0.4.0 changed the wire format (generation ids, delta ids, queued messages, new endpoints) and
0.5.0 moved the paths (`/api/v1` to `/api/client/v1`, `/pair` to `/pairings`, `/whoami` to `/me`)
and made every error a problem. 0.6.0 moved conversations, search, chat and generations under
`/spaces/{space_id}` (*Spaces* above), so `min_client` is 0.6.0. Before 1.0 paths may change like
this ([ADR 0022](adr/0022-api-path-conventions.md)); after 1.0, `v1` changes are additive only.

## Routing, retries and gates

- Only the provider asked for (`ChatRequest.provider`) serves the reply. There is no automatic
  fallback to another provider ([ADR 0014](adr/0014-model-gateway-and-providers.md)): if it fails,
  the reply ends with an `error` event, and the administrator adds or selects another provider.
  (A snapshot from an earlier version may still carry `fallback_order` and `allow_as_fallback`;
  both are ignored.) Provider entries carry `billing` (`metered` by default, or `subscription`)
  and per-million prices `price_in`, `price_out`, `price_cached_in` (optional; a missing price is
  an unknown cost, never zero).
- The provider is tried up to three times (once, then twice more) when the failure is worth it: a
  connection problem, a timeout, `408`, `429`, any `5xx`, or a stream that broke. The waits are
  250 ms and 1 s, or the provider's `Retry-After` when it is shorter than the time left. `401`,
  `403` and a missing key are not retried: the reply ends and the provider is flagged
  `auth_failed` until a reply from it succeeds or a new key is saved. Any other `4xx` ends the
  generation (it would be refused anywhere).
- **After the first delta the router never retries.** The person has seen text, so a
  failure then ends the generation with an `error` event and the text so far is kept.
- Everything above sits inside the generation's timeout (`generation_timeout_secs`).
- Gates run before anything starts. `kill_switch`: with the snapshot's `kill_switch` on, new
  generations are refused. When it goes on, queued messages are cleared, and replies already running
  are stopped (ending `cancelled`, `error_code: kill_switch`) when `kill_switch_aborts_running` is
  true (the default) or left to finish when false. `budget`: the tokens used since the start of the
  UTC day and month by metered providers are compared with `budget.daily_tokens` and
  `budget.monthly_tokens`; subscription providers count as zero. Each gate leaves a `gate` span
  (`done`, or `blocked` for a refusal, which gets a trace of its own).
- `GET /api/server/v1/providers/health` (server token): `{"providers": [{"provider_id", "status":
  "auth_failed", "since", "error_code"}]}`; only flagged providers are listed, and the reason is a
  code (e.g. `http_401`), never the provider's own text.
- Cost is computed whenever it is read, from the provider's prices in the snapshot at that moment:
  uncached input x `price_in` + cached x `price_cached_in` + output x `price_out`, per million
  tokens (OpenAI-style input counts include the cached tokens; Anthropic's do not). A subscription
  provider costs 0. A needed price that is missing gives `null`. The audit endpoints below carry it.
- Anthropic request bodies mark the system prompt and the older history cacheable; the cached
  tokens a provider reports are recorded on the attempt.

## Audit (server API)

The admin service reads what the server recorded through three endpoints on the server API
listener. They are not part of the client API: they need the server token, the client API
has no such routes (`404`), and the admin never opens `audit.db` itself. Everything they return
is **metadata** apart from the sealed payloads below, which are ciphertext the server cannot read.
A server with no audit store answers `503`.

- `GET /api/server/v1/audit/traces?status=&provider=&before=&limit=`: one summary per generation,
  newest first: `{"traces": [{"trace_id", "span_id", "status", "started_at", "ended_at", "user_id",
  "client_id", "tokens_in", "tokens_out", "tokens_cached", "error_code", "provider_id", "model",
  "ttft_ms", "latency_ms", "cost"}]}`. `status` filters on the turn's status, `provider` on the provider of
  its last attempt, `before` is a `trace_id` (only older traces, for paging), `limit` is 1 to 200
  (default 50). `trace_id` is the generation id.
- `GET /api/server/v1/audit/traces/{id}`: `{"trace_id", "cost", "spans": [...]}`, every span of that
  generation in the order they began (`404` if there is none). A span has `id`, `parent_id`,
  `kind`, `name`, `status`, `started_at`, `ended_at`, `provider_id`, `model`,
  `tokens_in|out|cached`, `ttft_ms`, `error_code`, `user_id`, `client_id`, `cost` and `attrs` (a
  JSON object; an attempt's carries `retry_index`, `latency_ms`, `http_status`; one recorded by an earlier
  version may also carry `fallback_from`).
  `gate` spans (`kill_switch`, `budget`, `queue`) end `done` when they let it through and `blocked`
  when they refused it.
- `GET /api/server/v1/audit/stats?since=<unix seconds>` (default: the last 24 hours):
  `{"since", "requests", "by_status": {...}, "error_rate", "latency_ms": {"p50","p95"},
  "ttft_ms": {"p50","p95"}, "tokens": {"in","out","cached"}, "cost", "cost_unknown_requests",
  "by_provider": [...], "by_day": [...]}` over generations that began at or after `since`.
  `error_rate` counts `error` and `timed_out`. `by_provider` and `by_day` (UTC `YYYY-MM-DD`) rows
  have `requests`, `tokens` and `cost`. A cost is `null`, and `cost_unknown_requests` says how many
  requests are the reason, when a price that was needed is missing.

- `GET /api/server/v1/audit/payloads/{span_id}`: `[{"role", "key_id", "ciphertext_b64"}]`, the sealed
  content of one span exactly as stored (`404` for a span that does not exist, `[]` for one with
  nothing sealed). `role` is `request` (on an attempt: the system prompt and history as JSON),
  `reply` (on a turn) or `error` (on an attempt: the provider's error text, cut at 4 KB). Request
  and reply are cut at 256 KB and the span then has `attrs.truncated = true`.

### The audit key

Prompts, replies and provider errors are stored in `audit.db` only as ciphertext, sealed (NaCl
sealed boxes, X25519) to a public key the administrator's passphrase produces. The server never
holds the private key and has no code to open a sealed payload.

- **Setup.** In the admin's first-run setup the administrator chooses a passphrase of at least 20
  characters (`PUT /api/admin/v1/setup/audit-key` with `passphrase` and `confirm`; `409` if a key
  exists). The admin stretches it with Argon2id (64 MiB, 3 passes, 1 lane) and a random 16-byte
  salt into 32 bytes, uses those as the X25519 secret key and keeps only the public key, the salt,
  the cost and the key id (the first 8 bytes of the SHA-256 of the public key, in hex).
- **The snapshot** carries `audit_public_key: {"key_id", "public_key"}` (32 bytes as 64 hex
  characters; the server checks that the id matches the key). Without it the server refuses new
  generations: `503 {"error":"audit_key_missing"}`.
- **Reading.** `POST /api/admin/v1/audit/payloads/{span_id}/open` with `{"passphrase"}`: the admin
  fetches the ciphertext, derives the key, opens the content and forgets the key. An
  `audit_access` row (span, key id, time, whether the passphrase was right) is written before any
  content is returned. A wrong passphrase is `403` and recorded; five wrong ones in 15 minutes lock
  opening out for 15 minutes (`429`).
- **Rotation.** `POST /api/admin/v1/audit/rotate-key` with `current`, `new` and `confirm`: a new key is
  made and pushed, the old one is retired but kept. Content is never re-sealed: each payload names
  its `key_id` and opens with the passphrase that made that key.

**Operator note.** The passphrase is not stored anywhere and cannot be recovered. If it is lost,
the content sealed with its key stays unreadable for good; the metadata (times, providers, models,
tokens, errors, statuses) stays available. After a rotation, the old passphrase is still needed to
read older content.

### audit.db

A separate SQLite file (`ASKBAR_AUDIT_DIR/audit.db`, the `audit-data` volume in the compose
setup), with its own migrations and `PRAGMA user_version`, written only by the server.

- `spans(id, trace_id, parent_id, kind, name, status, started_at, ended_at, provider_id, model,
  tokens_in, tokens_out, tokens_cached, ttft_ms, error_code, user_id, client_id, attrs_json)`.
  `kind` is `turn` (a whole generation), `gate` (a check or a wait, e.g. `queue`), `attempt` (one
  request to a provider), `tool` or `agent_step`. `trace_id` is the generation id; an attempt's
  parent is its turn. Times are unix seconds; an attempt's `attrs` carry `latency_ms`,
  `http_status`, `usage_estimated` and `finish_reason`. A span starts `running` and ends `done`,
  `error`, `cancelled`, `timed_out` or `interrupted` (the last for spans a restart cut off).
- `payloads(span_id, role, key_id, ciphertext, created_at)`: sealed content only (see *The audit key*).
- The database enforces its own rules with triggers: nothing younger than 7 days is deleted, and
  a span can change only while it is `running`. Records older than `audit_retention_days` (at
  least 7) are deleted once a day.

## Errors

Every failure of every Askbar API is `application/problem+json` (RFC 9457):

```json
{"type": "urn:askbar:problem:budget_exceeded", "title": "A usage limit has been reached",
 "status": 429, "detail": "The usage limit for today has been reached. ...",
 "error": "budget_exceeded", "period": "daily"}
```

`error` is the stable code a program branches on; `title` is fixed per code; `detail` is a
sentence for a person; extra members such as `period` ride alongside. Status codes and
`Retry-After` are part of the contract. The codes (`crates/askbar-proto/src/problem.rs`):

The full list of codes, with their statuses, is the `error` enum of the `Problem` schema in each
OpenAPI file below; the source is `crates/askbar-proto/src/problem.rs`. A test keeps the files and the code the
same.

A refusal never starts a
generation and never loses the typed text. Caddy answers a path nothing serves with the same
`not_found` problem.

(There is no setup mode: the server creates its master key on first start.)

**Not configured yet.** Until the admin has pushed configuration for the first time, every client
route except `GET /health` (and the old `/api/v1`) answers 503 `not_configured` with
`Retry-After: 5` ([ADR 0028](adr/0028-service-boundaries-and-config-delivery.md) rule 3). The
order is the version gate, then this, then the token: an app is never told its token is refused
while the server waits for its settings. The configuration is stored in `control.db`, so a
restarted server stays configured; only one on an empty volume answers this, for the few seconds
until the admin's next status check (every 12 s) pushes it.

## What the desktop client does with them

- The app tells a refusal by its `error` code, never by its words: `queue_full`,
  `budget_exceeded` (`period` picks "today" or "this month"), `kill_switch` and
  `audit_key_missing` each show their own sentence and keep the message in the outbox; nothing is
  retried by itself.
- `audit_backlog` and `not_configured` (with `Retry-After`) are tried once more after that many
  seconds; if it is refused again the message waits for the person. The status check shows a 503
  as "setup pending" and keeps the token.
- A reply whose `error` event has `code` `timed_out` is shown as "Stopped after ..." with the text
  that had arrived, and the app offers to ask again as a new reply.
