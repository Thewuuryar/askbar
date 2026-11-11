# 0054. The `model-gateway` component

Status: Accepted
Date: 2026-11-10
Kind: component
Source: plan step 3.1 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07); owner's order of 2026-11-10 (`model-gateway` before the server sends records to the auditor)

## Context

[0014](0014-model-gateway-and-providers.md) decides that `model-gateway` holds the provider keys in memory, enforces budgets and the approved model list, and makes every provider call, so the server needs neither keys nor an outbound route. [0010](0010-auditor.md) rule 2 makes it the only opener of model-call spans. Today the server holds the keys, calls providers itself, and records to its own `audit.db` ([0039](0039-server.md)). The auditor is built and waits for its callers ([0051](0051-auditor.md)). This record describes the container that plan step 3.1 builds, and that steps 3.2 to 3.5 extend by amendment: budgets and the kill switch, the model catalog, the streaming provider test, and `cpa`.

## Decision

- **What it is:** `services/model-gateway`, image `askbar-model-gateway`, serving `/api/model-gateway/v1` ([0022](0022-api-path-conventions.md)). It links `askbar-service` ([0036](0036-askbar-service-crate.md)) and `askbar-proto`, and reaches the auditor over HTTP, so it links no store code. The provider-calling code moves into it from the server's `llm.rs`. No new third-party dependency.
- **What it holds:** nothing on disk. It has no data volume.
  - **Provider keys** live in memory only. The admin stores them (encrypted with its own key, [0040](0040-admin.md)) and pushes them in `PUT /config`. No endpoint returns a key, and no log, error, audit record or test output holds one.
  - **Configuration:** the providers (kind, address, headers) and the audit public key. Budgets, the model catalog and `cpa` arrive in later steps.
  - **Credentials:** its own volume, `model-gateway-token`, written by `bootstrap` ([0049](0049-bootstrap.md)): the hashes it accepts and the token it sends to the auditor.
- **Start and restart:** it starts unconfigured and answers `503 not_configured` until the admin pushes ([0028](0028-service-boundaries-and-config-delivery.md) rules 3 and 4). The admin pushes again on a new boot ID, so a restart or a reboot needs no command. Because nothing is on disk, a restart is how a key leaves its memory.
- **Its API, by link:**
  - the admin: `GET /status`, `PUT /config`, `POST /restart`, and later the provider test (step 3.4);
  - the server: the streaming chat endpoint, `POST /chat`, and later the approved model list (step 3.3).
  - Each token hash maps to one caller and its routes; anything else is `403`. The wire types live in `askbar-proto` ([0045](0045-askbar-proto-crate.md)) and the routes in `docs/openapi/model-gateway.v1.yaml`.
- **A model call** ([0010](0010-auditor.md) rules 1 to 3):
  1. The server sends `POST /chat` with the user, space, conversation and generation (turn) ids, the model, and the messages.
  2. The gateway appends the turn span's start to the auditor and waits for the acknowledgment, 1 s at most ([0051](0051-auditor.md) table). It calls no provider before that.
  3. It answers with a Server-Sent Events stream. The first event carries the span ID; then come the deltas and usage, then `done` or `error` ([0045](0045-askbar-proto-crate.md) `sse`).
  4. For each try at a provider it opens an attempt span under the turn, and finishes it when the try ends. Retries are attempts under one turn.
  5. The server adds its sealed payloads (the request and the reply) and the turn's finish record to the span ID it received, through its own auditor token, with a bounded retry queue (0010 rule 5).
- **A refusal:** if the auditor is unreachable, does not acknowledge in time, or refuses (`503 audit_backlog`), the gateway answers `503 audit_backlog` with a retry hint and calls no provider. It keeps a bounded queue of refusal records in memory (1,000 at most; the oldest is dropped, and the count of drops is recorded), sent when the auditor is back. That queue is lost if the gateway restarts, which is accepted.
- **A turn the server never finishes:** the turn span belongs to the gateway's token, so the auditor's boot-ID rule ([0010](0010-auditor.md) rule 6) ends it only when the *gateway* restarts. A span the server's finish never reaches (the server crashed) would stay running. The gateway therefore finishes any turn span still open after the generation time limit plus 60 seconds as `interrupted`. The first finish of a span is the one that counts, so a late finish from the server changes nothing.
- **Provider errors and content:** only the gateway sees a provider's error text. It never logs it and never sends it to the server, which gets an error class and a stable code. It seals the text to the audit public key it was pushed and appends it as a payload on the attempt span ([0011](0011-sealed-content-access.md)). The gateway has no private key and no function that opens a payload.
- **Links and tokens** ([0049](0049-bootstrap.md), [architecture.md](../architecture.md), Links):
  - `server-model-gateway`: the server calls the gateway; the token is in `server-token/call/`, the hash in `model-gateway-token/accept/`;
  - `admin-model-gateway`: the admin pushes configuration, reads status and restarts it;
  - `model-gateway-auditor`: the gateway appends to the auditor, with the scopes 0051 gives it (any kind of span, finish, payload, hello).
  - Prefixes are `srvgw_`, `admgw_` and `gwaud_`. The `cpa` link is not a token of ours: `cpa` compares a plain API key, which the admin pushes with the provider (0049, "Not in scope").
- **Networks and outbound** ([0006](0006-v1-outbound-access-and-alerts.md)): direct outbound in v1, no egress proxy. One network for each link, shared with the gateway alone, none pinned to a subnet:
  - `gateway-server` and `gateway-admin`, `internal`;
  - `audit-model-gateway`, `internal`, shared with the auditor;
  - `outbound`, not `internal`: the providers, and in dev the mock LLM. It joins no one else in this step. `cpa` joins a gateway network, `cpa-gateway`, in step 3.5.
- **Listeners:** two, one on each caller's network, each bound to the address its alias resolves to on that network (`gateway-server-host`, `gateway-admin-host`), and never a wildcard, because the gateway also sits on `outbound`. Each accepts only its own link's token, so a token cannot be used on the wrong network.
- **Who it runs as:** uid 10004 ([0003](0003-v1-service-layout.md)), from its first instruction.
- **Hardening:** [0032](0032-container-hardening.md)'s defaults with no exception: read-only root, no capabilities, a private `/tmp`, memory 256 MB. `check-isolation.sh` checks:
  - the defaults, `CapEff` 0, and uid 10004;
  - its networks are exactly those above, and only `outbound` has a route out;
  - it mounts `model-gateway-token` and nothing else;
  - the server has no network with a route out, and no key reaches it;
  - a marker key pushed to the gateway never appears in its logs, its filesystem, any response, or the auditor's store.
- **Changes elsewhere when built (plan step 3.1, two pull requests):**
  - **The server** ([0039](0039-server.md), amended): stops holding keys and calling providers (`secrets.rs`, the provider code, `master.key`), stops mounting `askbar-secrets` and `audit-data` (both stay declared), records to the auditor, and moves to internal networks only. That needs a network for Caddy and the server alone, because `edge` has a route out.
  - **The admin** ([0040](0040-admin.md), amended): stores provider keys and pushes them. Keys already in the server's database cannot be read by the admin: **they are entered again after this upgrade** (the guide says so).
  - **The auditor** ([0051](0051-auditor.md), amended): the `audit-server` and `audit-model-gateway` networks and the two links.
  - `mock-llm` joins `outbound` in the dev profile.

## Alternatives considered

- **Keeping the keys on a volume in the gateway:** rejected. A stolen volume would hold them ([0028](0028-service-boundaries-and-config-delivery.md)); memory plus a re-push after a restart costs one dependency on the admin, which already pushes everything.
- **The gateway pulling its configuration from the admin:** rejected in [0028](0028-service-boundaries-and-config-delivery.md).
- **The server opening spans, with a token the auditor signs for the gateway to check:** rejected in [0010](0010-auditor.md).
- **The gateway finishing the turn span itself:** rejected here. The server alone knows when the reply was stored, and the server's finish record is what 0010 specifies; the gateway's time-out only covers a server that never finishes.
- **One listener on every interface:** rejected, because the gateway joins `outbound`; a peer there would reach a port it has no link to.
- **Putting `outbound` on the existing `edge` network:** rejected. `edge` is Caddy's, and the server must leave it.

## Consequences

- Every reply passes through one more hop and one auditor round trip before the provider call; the acknowledgment counts toward the 200 ms budget ([0002](0002-v1-scope-limits-and-ship-criteria.md), [0010](0010-auditor.md) rule 4).
- If the auditor is down or full, model work stops ([0010](0010-auditor.md)). If the admin is down after a gateway restart, so does it, until the admin pushes the keys again.
- A compromised gateway holds the keys in memory and has direct outbound access until the egress proxy ships, so it can send them out ([0006](0006-v1-outbound-access-and-alerts.md), [0016](0016-risk-register.md)). It can already reach providers; the independent records and alerts of 0016 cover a skipped span start.
- A compromised server can misuse model access only within what the gateway allows: its budgets, providers and approved models ([0014](0014-model-gateway-and-providers.md)).
- Upgrading from today's install means entering the provider keys again.

## Amendments
- 2026-11-10: built in plan step 3.1, first half (PR: feat/model-gateway), standing alone: the server does not call it yet. Built so: both listeners use port 8084 (`ASKBAR_GATEWAY_SERVER_ADDR` and `ASKBAR_GATEWAY_ADMIN_ADDR` name their aliases); the retry loop (three tries, none once text has gone out) moved into the gateway with the provider code, and the server keeps its own until it calls the gateway; every try's attempt span is acknowledged before the request, as the turn's is; records that need no acknowledgment (finish records, payloads, refusals, and one record of how many were dropped) share one in-memory queue of 1,000 that is drained in order; no span is opened until the auditor has taken the gateway's hello; a configuration's version is a hash of its content, so the admin keeps no counter; the admin checks every 12 seconds, and pushing on a new boot ID seen in the auditor waits for the admin's auditor reads (plan step 2.5); a push that fails does not block an admin change until the server depends on it. Not built: the server's side, budgets, the model catalog, the provider test, and the `cpa` network.
- 2026-11-11: the server calls it (plan step 3.1, second half). Built so, beyond the first half: the model list and the provider test moved here from the server as the admin's `POST /models` and `POST /providers/{id}/test` (decided with the owner; steps 3.3 and 3.4 extend them), so the gateway's admin listener has five routes; the shared auditor client moved to `askbar-service` and the server uses it for its own records; `cpa-gateway` (`internal`, `cpa` and the gateway) exists now, not in step 3.5, because the server no longer reaches `cpa` and a subscription provider would otherwise stop working; the server's network for Caddy is `client-api`. A provider's health flag, set when the gateway reports a refused key, clears on the next reply that works, not when a key is entered again. Upgrading means entering each provider's key again, as the Consequences say.
- 2026-11-11: records the configuration version it applied with `model-gateway-auditor` (`POST /config-applied`), through the queue that retries until the auditor takes it (plan step 2.7, [0028](0028-service-boundaries-and-config-delivery.md) rule 5, [0051](0051-auditor.md)).
