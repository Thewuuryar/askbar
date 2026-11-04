# Logging and audit

Two separate records, with different readers and different rules. Never use one in place of the other.

## Operational logs

What a person running the stack reads in `docker compose logs`. Written with `tracing`. Existing code uses `info` and `warn`; the levels below are the **(proposed)** standard for all new code.

- **Metadata only.** Ids, counts, durations, status codes, versions, addresses. Never message content (prompts, replies, provider errors), provider keys, tokens, the audit passphrase, pasted login addresses, the Cloudflare token or the admin password. Do not log query strings or headers.
- **Structured fields, short message.** Put values in fields and keep the message a fixed phrase, as the existing lines do: `info!(%addr, "listening")`, `info!(version = v, "snapshot pushed")`.
- **Levels.**
  - `info`: lifecycle and normal housekeeping (listening, snapshot loaded, records pruned).
  - `warn`: something failed and the service carried on (an audit write, retention, pruning, a Caddy call at start-up).
  - `error`: the service cannot do what it is for.
  - `debug` and `trace`: off by default; still never secrets or content.
- **Errors reach people as problems, not as log text.** A caller gets a problem with a stable `error` code. The log line says what happened to the server; it does not copy the body of an upstream error.

## The audit trace

The record of what the system did with a reply, kept for investigation. Described for users in README ("The Audit tab"); the boundaries are in [architecture.md](../architecture.md).

- **Every reply is recorded**: when, for whom, which providers were tried, step timings, tokens, outcome. Any new code path that calls a model or provider must record it too.
- **Content is sealed.** Prompts, replies and provider errors are stored only as ciphertext made with the public audit key. The server cannot open them. Opening is done by the admin with the passphrase, and each opening is written to the access history (the request, the key, the time; never the content).
- **Retention.** Default 30 days, setting floor 7. Nothing younger than 7 days is deleted. A lost passphrase means sealed content is gone; the metadata stays readable.
- **No key, no replies.** Until an audit key exists the server answers `503 audit_key_missing` rather than record unsealed.
- **Only the server mounts `audit.db`.** The admin reads it through the server API.
- **Admin actions.** Starting, stopping or restarting a service writes a `service_actions` row in `admin.db` (service, action, time, whether it worked). It does not record who did it. **(proposed)** Every admin action that changes state writes a row that also says who did it (a session or the API key), and opening sealed content stays in the access history.

## Later

If the model is ever given tools ([ADR 0007](../adr/0007-conversation-only-and-tool-gating.md)): each tool call, command, network request and file change is an audit span written **before** it runs, and a failed audit write stops the call.
