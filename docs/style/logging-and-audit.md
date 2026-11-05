# Logging and audit

Two separate records, with different readers and different rules. Never use one in place of the other.

## Operational logs

What a person running the stack reads in `docker compose logs`. Written with `tracing`. Existing code uses `info` and `warn`; the levels below are the **(proposed)** standard for all new code.

- **Metadata only.** Ids, counts, durations, status codes, versions, addresses. Never message content (prompts, replies, provider errors), provider keys, tokens, the audit passphrase, pasted login addresses, the Cloudflare token or the admin password. Do not log query strings or headers. Alerts never include a token ([ADR 0024](../adr/0024-rejected-token-alert.md)).
- **Structured fields, short message.** Put values in fields and keep the message a fixed phrase, as the existing lines do: `info!(%addr, "listening")`, `info!(version = v, "snapshot pushed")`.
- **Levels.**
  - `info`: lifecycle and normal housekeeping (listening, snapshot loaded, records pruned).
  - `warn`: something failed and the service carried on (a finish record queued for retry, retention, pruning, a Caddy call at start-up).
  - `error`: the service cannot do what it is for (for example, the auditor is unreachable, so model calls are refused).
  - `debug` and `trace`: off by default; still never secrets or content.
- **Errors reach people as problems, not as log text.** A caller gets a problem with a stable `error` code. The log line says what happened to the server; it does not copy the body of an upstream error.

## The audit trace

The record of what the system did with a reply, kept for investigation. Described for users in README ("The Audit tab"); the boundaries are in [architecture.md](../architecture.md).

- **Every reply is recorded**: when, for whom, in which space, which providers were tried, step timings, tokens, outcome. Any new code path that calls a model or provider must record it too. Only `model-gateway` opens model-call spans, and a provider call doesn't start until the auditor acknowledges its span ([ADR 0031](../adr/0031-audit-span-start-and-backlog.md)).
- **Content is sealed.** Prompts, replies and provider errors are stored only as ciphertext made with the public audit key. The server cannot open them. Opening is done by the admin with the passphrase. Each session start, end and item viewed is recorded in the auditor and sends an alert; never the content ([ADR 0011](../adr/0011-sealed-content-access.md)).
- **Retention.** Default 30 days, setting floor 7. Nothing younger than 7 days is deleted. A lost passphrase means sealed content is gone; the metadata stays readable.
- **No key, no replies.** Until an audit key exists the server answers `503 audit_key_missing` rather than record unsealed.
- **Auditor down or behind.** New replies are refused with `503 audit_backlog` ([ADR 0031](../adr/0031-audit-span-start-and-backlog.md)).
- **Crashes.** A service queues finish and payload records and retries them. Spans left unfinished after a crash are marked interrupted when the service reconnects with a new boot ID ([ADR 0025](../adr/0025-audit-record-reliability.md)).
- **Only the auditor mounts the audit store.** The admin reads it through the auditor API ([ADR 0010](../adr/0010-auditor.md)). On `main` the server still holds `audit.db`.
- **Admin actions.** They are appended to the auditor in the categories in [ADR 0010](../adr/0010-auditor.md), and the admin refuses an action if its record can't be appended. **(proposed)** Each record says which credential made the change (a session or the API key). On `main`, starting, stopping or restarting a service writes a `service_actions` row in `admin.db` instead, without who did it.

## Later

If the model is ever given tools ([ADR 0002](../adr/0002-v1-scope-limits-and-ship-criteria.md), [ADR 0019](../adr/0019-post-v1-order-and-tools.md)): each tool call, command, network request and file change is an audit span written **before** it runs, and a failed audit write stops the call.
