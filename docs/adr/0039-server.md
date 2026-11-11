# 0039. The `server` component

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan step 1.14 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

The server existed before the rule that each component has its own ADR. This record describes it as it is on `main` after plan phase 3. The decisions it follows are in [0003](0003-v1-service-layout.md), [0005](0005-no-controller-and-self-restart.md), [0008](0008-users-clients-and-client-access.md), [0009](0009-spaces-and-storage.md), [0022](0022-api-path-conventions.md), [0023](0023-api-error-format.md), [0028](0028-service-boundaries-and-config-delivery.md) and [0032](0032-container-hardening.md); this record links them instead of repeating them.

## Decision

- **What it is:** `services/server`, binary and image `askbar-server`, the Dockerfile's `server` target. It runs as uid 10002 and serves chats for paired clients.
- **Two listeners:**
  - the client API, `/api/client/v1`, on port 8080 of the `client-api` network, reached through Caddy;
  - the server API, `/api/server/v1`, on the server's own address on the `server-api` network, found by the alias `server-api-host` that exists on that network only (port 8082), for the admin only. It refuses a wildcard address, or a name that resolves to more than one.
- **What it holds:**
  - `control.db`: users, clients (token hashes only), pairing-code hashes, spaces and memberships, the snapshot the admin pushed (with the budget copy), token usage and provider health;
  - one file per space under `spaces/` ([0009](0009-spaces-and-storage.md)), holding its conversations. Each reply keeps a copy of the provider name and model that wrote it, and a question whose turn failed keeps why, in the server's own words;
  - no provider key and no audit store: `model-gateway` holds the keys ([0054](0054-model-gateway.md)) and the auditor keeps `audit.db` ([0051](0051-auditor.md)). The server seals each request and reply with the public half of the audit key it was pushed ([0011](0011-sealed-content-access.md)) and sends them on.
- **Volumes:** `DATA_DIR` (a host directory, `./data` by default) at `/data`, and `server-token` read-only at `/server-token` (the hash of the admin's token, `accept/admin-server.sha256`, and the hashes of the other links it accepts, made by `bootstrap`). Nothing else is mounted.
- **Networks:** `client-api` (Caddy), `server-api` (the admin), `gateway-server` (`model-gateway`) and `audit-server` (the auditor). All are `internal`, so the server has no route out. It publishes no port.
- **Links:** Caddy calls the client API with a client token. The admin calls the server API with its token, checked against `server-token/accept/admin-server.sha256` through `askbar-service` ([0036](0036-askbar-service-crate.md)). The server calls `model-gateway` (`server-model-gateway`) and the auditor (`server-auditor`), and no provider and no `cpa`. It never calls the admin.
- **Configuration:** it answers `503 not_configured` until the admin pushes a snapshot, then keeps it in `control.db` ([0028](0028-service-boundaries-and-config-delivery.md)). The snapshot's settings apply as each reply starts: the time limit, and `history_turns`, how many earlier questions go with a new one. It restarts itself on `POST /api/server/v1/restart` and when its own health check keeps failing ([0005](0005-no-controller-and-self-restart.md)).
- **From plan step 5.1 (privacy):** the snapshot carries a `policy` (the privacy notice with its version and the AI warning, [0012](0012-consent-notices-and-deletion.md)), and the server serves it to clients on the client API (additive; `MIN_CLIENT` does not change). If 0012 requires consent to be recorded, an additive route records each acknowledgment.
- **From plan step 5.1 (deleting a user, with the privacy notice):** the server API deletes a user: the personal space file, the user's clients and their token hashes, and the user row. Audit records stay. The admin calls it first, then removes its own row ([0040](0040-admin.md)).
- **From plan step 6.2 (backups):** `POST /exports` streams `control.db` and the space files encrypted to the backup public key (`age`, from the snapshot), with temporary files on `/data`, and answers `503` while there is no key. `POST /imports` takes a restore, and is refused with `409 not_empty` unless the install is empty ([0013](0013-backups.md)). The server never holds the private key.
- **Hardening:** the shared defaults of [0032](0032-container-hardening.md), with no exception; memory limit 512 MB.

## Alternatives considered

Not recorded: the server predates this record.

## Consequences

`check-isolation.sh` tests what the server can and cannot reach in the running stack, and the server's own tests cover its APIs, storage and audit trail.

**Where the code and an accepted ADR disagree.** Two differences are left; neither is a decision to keep it:

- [0003](0003-v1-service-layout.md) (no host bind mounts): the data directory is a host bind mount.
- [0013](0013-backups.md): the backup export is not encrypted, so it leaves secrets out (phase 6).

## Amendments

- 2026-11-08: the server API binds the address of the alias `server-api-host` instead of the fixed `172.30.0.2`; no subnet or address is pinned (PR #32, [0051](0051-auditor.md)).
- 2026-11-08: each reply in a space file keeps a copy of the provider and model that wrote it (space migration 2; plan step 1.13).
- 2026-11-10: a question whose turn failed keeps why, in the server's own words, never a provider's error text (space migration 3; plan step 1.13).
- 2026-11-10: the snapshot's `history_turns` limits how many earlier questions, with their replies, are sent with a new one (0 = all; plan step 1.13).
- 2026-11-10: reads its accepted hash from `server-token/accept/admin-server.sha256`, made by `bootstrap`, and refuses to start without it; an install with only the old `token.sha256` is told to run `docker compose up -d` (plan step 2.1, [0049](0049-bootstrap.md)).
- 2026-11-10: counts tokens that match no link on the server API and logs once when five arrive within ten minutes, naming the link, never the token (plan step 2.2, [0024](0024-rejected-token-alert.md)).
- 2026-11-11: holds no provider key and calls no provider (plan step 3.1, second half, [0054](0054-model-gateway.md)). Built so: it asks `model-gateway` for each reply over `gateway-server` with its `server-model-gateway` token and follows the stream: the first event names the turn span the gateway opened, the server adds its sealed request and reply and the turn's finish to it, and a failure reaches the person as the server's own sentence (never the provider's words) with a code. Its records go to the auditor through a bounded queue; it answers `503 audit_backlog` to a new generation when the auditor has not taken its boot ID, refused or did not answer its last request, or the queue is nearly full. Removed: `secrets.rs` and the master key, the provider calls, the retry router, the routes that set a key, try a provider or list models (the admin asks the gateway), `missing_key`, the in-process audit store and its retention job, and the mounts of `askbar-secrets` and `audit-data` (both stay declared). The server API's `/audit/*` reads the auditor with a temporary link and adds its prices, until the pages move to the admin (plan step 2.5). Networks: it leaves `edge` for `client-api` (Caddy and the server), and joins `gateway-server` and `audit-server`; none has a route out.
- 2026-11-11: no longer serves the audit store: `/api/server/v1/audit/*`, the temporary read link, the price code and the cost columns are removed, and the admin reads the auditor itself (plan step 2.5, [0051](0051-auditor.md)). The snapshot still carries `audit_retention_days`, which the server ignores.
- 2026-11-11: records the configuration version it applied with `server-auditor` (`POST /config-applied`), through the queue that retries until the auditor takes it (plan step 2.7, [0028](0028-service-boundaries-and-config-delivery.md) rule 5, [0051](0051-auditor.md)).
- 2026-11-11: refuses a paused user's new reply early with `403 user_paused` and a `user_pause` gate span, from `paused_user_ids` in the snapshot (plan step 3.2); the gateway's copy is authoritative and also stops the replies running. A refusal or stop that comes from the gateway (`kill_switch`, `user_paused`, `budget_exceeded`) ends the generation with that code. The day and month start moved to `askbar-service`.
- 2026-11-12: offers the approved models and checks a send against them (plan step 3.3, [0014](0014-model-gateway-and-providers.md), [0054](0054-model-gateway.md)). Built so: `GET /api/client/v1/models` replaces `/providers` and returns what the gateway's `GET /catalog` says (kept five seconds); a send names a model (`provider/model`) or none, which means the default, and one outside the catalog is `400 model_not_approved`; the preference is `active_model` (control migration 9; `active_provider_id` stays in the table, unused); a conversation stores the model its latest message asked for (space migration 4) and shows it as `Conversation.model`; a generation row holds the model asked for from the start. Nothing approved yet means no send is accepted.
- 2026-11-11: body brought up to date with the code on `main`: no provider keys and no `audit.db`, the volumes and networks as built, the links to `model-gateway` and the auditor; the disagreements with 0003, 0010, 0014 and 0028 rule 5 are resolved and removed. No decision changed.
- 2026-11-11: design for plan steps 5.1, 6.2 and 6.3 (#70, owner's decisions D15, D16, D22, D23 and D25): the policy in the snapshot, deleting a user, `POST /exports` encrypted to the backup public key in the snapshot, and `POST /imports` into an empty install. The code PRs add the 'built' lines.
