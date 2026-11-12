# 0051. The `auditor` component

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan steps 2.4 to 2.9 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07); owner review of the open questions (2026-11-08)

## Context

[0010](0010-auditor.md) decides the audit rules: append-only records, narrow tokens, span start, backlog, record reliability and the hash chain. [0011](0011-sealed-content-access.md) decides how content is sealed. `audit.db` lives in the auditor's own container and volume; the server, `model-gateway`, the admin and Caddy reach it over HTTP or its ingest listener, and none opens the file ([0039](0039-server.md)). This record describes that container, built in plan steps 2.4 to 2.9 around the `askbar-audit` crate ([0052](0052-askbar-audit-crate.md)).

## Decision

- **What it is:** `services/auditor`, image `askbar-auditor`. It serves `/api/audit/v1` ([0022](0022-api-path-conventions.md)) and links `askbar-audit` and `askbar-service` ([0036](0036-askbar-service-crate.md)).
- **What it holds:**
  - `audit.db` on a new volume, `auditor-data`. It starts empty: nothing needs migrating before 1.0 ([0018](0018-releases-and-migrations.md));
  - its callers' token hashes, in `auditor-token`, written by `bootstrap` ([0049](0049-bootstrap.md));
  - metadata and sealed payloads only. It never holds the passphrase or the private key, and has no function that opens a payload.
- **Its API, by link** ([0010](0010-auditor.md)). `LINKS` in `services/auditor/src/lib.rs` is the list; each link is live once `bootstrap` has made its token hash:
  - `server-auditor` and `model-gateway-auditor` (service append): `POST /hello`, `/spans`, `/spans/*/finish`, `/spans/*/payloads` and `/config-applied`, and `GET /status`. The server's link opens `gate` spans only; the gateway's any kind;
  - `admin-auditor-append` (admin append): `POST /hello` and `/admin-actions`, and `GET /status`;
  - `admin-auditor-read` (admin read): `GET /status`, `/traces`, `/traces/*`, `/spans/*/payloads`, `/stats`, `/token-counts`, `/admin-actions`, `/prunes`, `/access-logs`, `/chain/head`, `/chain/entries/*` and `/chain/verify`, and `GET` and `PUT /retention`;
  - `model-gateway-auditor-read` (`gwrd_`, caller `model-gateway-read`): `GET /status` and `/token-counts`, so the gateway can hold its budget against what was used. It appends nothing;
  - `alerter-auditor` (`alraud_`, caller `alerter`; built, but live only once `bootstrap` makes its hash in plan step 4.1's stack PR): `POST /hello`, `/config-applied` and `/alert-deliveries` (the outcome of each alert, metadata only, chained), and `GET /status`;
  - `admin-auditor-read` also gets `GET /alert-deliveries` (built) and, from plan steps 6.2, `PUT /backup-key` (the backup public key, recorded as a chained admin action record) and `GET /export` (an encrypted snapshot, streamed);
  - **from plan step 6.3:** `admin-auditor-restore` (`audrs_`, caller `admin-restore`): `POST /restore` and nothing else. It works only on an empty store and verifies the chain first ([0010](0010-auditor.md)'s one exception to append-only, [0013](0013-backups.md)).
  - Each token hash maps to one caller and the routes it may call; anything else is `403`. The caller comes from the token, never from the body.
  - No token changes or deletes a record.
  - The wire types live in `askbar-proto` ([0045](0045-askbar-proto-crate.md)).
- **What it does beyond 0010's rules:**
  - **Prune records:** each prune is a chained record, written in the same transaction as the delete. It holds the cutoff and the counts of spans and payloads removed. The chain entry of the prune is what a chain check finds in place of the deleted records, and the admin sees every prune.
  - **Token counts:** it serves them by provider and model to the admin read token. The admin applies prices; the auditor holds none.
  - **No key registry:** the admin keeps it and appends each key's creation and retirement as admin action records, so its history is in the chain ([0011](0011-sealed-content-access.md) rule 6).
  - **Alert rules (from plan step 4.2, [0055](0055-alerter.md)):** a rules engine fed after each append, after each request on a link and every 30 seconds sends events to `alerter` over `auditor-alerter` (`audalr_`, `call/auditor-alerter.token` in `auditor-token`), with typed fields only. Its kinds: `unsealed`, `unseal_timeouts_changed` and `backup_key_changed` (from the admin action records), `retention_changed` (from `PUT /retention`), `audit_disk_low` and `audit_disk_critical`, `component_silent`, `caddy_silent`, and `restart_loop` for the admin (from its hellos).
  - **Silence:** it records the time of the last request on each link. A link is silent when nothing has come over it for 5 minutes; `AuditClient` ([0036](0036-askbar-service-crate.md)) calls `GET /status` every 60 seconds, so a quiet but alive service is not silent. Caddy is silent when a turn was recorded in the last 15 minutes and Caddy sent no access line in that time. The last request on each link is in `GET /status`.
  - **Restart loop:** three boot-ID changes within 10 minutes. Caddy and `cpa` are excluded.
  - **Disk:** `audit_disk_low` below 1 GiB free, re-armed above 1.25 GiB; `audit_disk_critical` below 256 MiB.
  - **From plan step 6.2:** a read-only encrypted export, made with `snapshot_to` (`VACUUM INTO` on its own volume, after a free-space check) and encrypted to the backup public key the admin puts with `PUT /backup-key`; and, from plan step 6.3, a restore into an empty store.
  - **Hash chain and Caddy's access logs:** every record, including each prune and each access line, has a chain entry ([0052](0052-askbar-audit-crate.md)); the admin reads the chain on `admin-auditor-read`.
- **Starting limits,** tuned by the load test of plan step 7.2:

  | Setting | Default |
  | --- | --- |
  | Acknowledgment timeout at `model-gateway` | 1 s |
  | Backlog limit | 1,000 queued records, or the oldest older than 5 s |
  | Disk early warning (alerted from plan step 4.2) | below 1 GiB free, re-armed above 1.25 GiB; critical below 256 MiB |
  | Refuse new spans | below 256 MiB free, leaving room for the finish and payload records of open spans |

- **Volumes:**
  - `auditor-data`: Docker creates it empty and owned by uid 10003, because the image's `/audit` belongs to that uid.
  - `auditor-token`: its credential volume, read-only.
  - The server's old `audit-data` is neither renamed nor mounted. It stays declared until 1.0 and holds the pre-2.4 records until the owner copies or deletes it (the guide says how).
- **Who it runs as:** uid 10003 ([0003](0003-v1-service-layout.md)), from its first instruction, with no capabilities.
- **Networks:** internal only, with no outbound route. There is one network per caller, shared with the auditor alone: `audit-admin`, `audit-server`, `audit-model-gateway` and `audit-caddy` (Caddy's access lines). From plan step 4.1, `audit-alerter` joins them, with the alias `auditor-alerter-host`. No subnet is pinned ([guide](../guide.md#docker-networks)).
- **Listener:** one API address for each token network, so a caller reaches only its own: `auditor-admin-host`, `auditor-server-host` and `auditor-model-gateway-host` and, from plan step 4.1, `auditor-alerter-host`, all on port 8083 (`ASKBAR_AUDIT_API_ADDRS`). A second listener, `auditor-ingest-host:8086` on `audit-caddy`, takes Caddy's access lines, sends no token and answers nothing (`ASKBAR_AUDIT_INGEST_ADDR`). Nothing on `audit-caddy` reaches the API.
- **Links** ([architecture.md](../architecture.md), Links):
  - in: from the server, `model-gateway` (append and read), the admin (append and read), and Caddy (access lines to the ingest listener, no token);
  - in, from plan step 4.1: the alerter (`alerter-auditor`); and, from plan step 6.3, the admin's restore link;
  - out: from plan step 4.2, only to `alerter` (`auditor-alerter`).
- **Hardening:** [0032](0032-container-hardening.md)'s defaults, with no exception. `check-isolation.sh` checks:
  - `CapEff` is 0;
  - there is no route out;
  - each caller reaches it only on its own network, and its networks are exactly the `audit-*` ones;
  - no unsealed content reaches its store.

## Alternatives considered

- **One shared audit network:** rejected. Callers could reach each other's ports; tokens would refuse them, but reach would no longer follow the Links table.
- **Running as the server's uid:** rejected. Each service has its own uid.
- **Keeping `audit-data` and changing its owner at start-up:** rejected. It needs a capability exception to 0032, for a migration 0018 says isn't needed; a host command instead adds a manual upgrade step.
- **No prune record:** rejected. Deleting the oldest records would look like retention.
- **An auditor copy of the key registry:** rejected. The admin needs it to open records, and two copies could disagree.

The decisions behind the auditor itself (its own container, synchronous acknowledgment, no off-host chain head) are in [0010](0010-auditor.md).

## Consequences

- If the auditor is down or full, model work stops ([0010](0010-auditor.md)).
- A compromised caller can still append false records or leave some out; the chain shows rewrites and deletions, not lies ([0016](0016-risk-register.md)).
- Each caller adds a network (about fifteen by phase 4), and each takes one of Docker's address pools; the [guide](../guide.md#docker-networks) covers a host that runs out.
- The audit trail restarts at step 2.4: older records stay only in the unmounted `audit-data`.

## Amendments

- 2026-11-10: trimmed: decisions that 0010 and 0011 make are linked instead of restated (token scopes, acknowledgment and latency, boot IDs, retention, backlog), and links to the merged 0025, 0030, 0031 and 0046 point to 0010, 0011 and 0001. No decision changed.
- 2026-11-10: built standing alone in plan step 2.4a (#56): nothing sends it records until step 3.1, and the server still records to its own file. Built so: its links are `server-auditor`, `model-gateway-auditor`, `admin-auditor-append` and `admin-auditor-read` (only the two admin links have tokens from `bootstrap` yet, [0049](0049-bootstrap.md)); the server's link opens `gate` spans only and `model-gateway`'s any kind; a hello ends that service's earlier unfinished spans as interrupted; admin action records and prune records are tables of their own, never changed or deleted (a prune record holds the cutoff and the counts; the hash of the last record deleted comes with the chain, plan step 2.6); a backlog over 1,000 queued records or an oldest record waiting over 5 s refuses every append, and below 256 MiB free a new span, with `503 audit_backlog`; free space is read from `df`; the network `audit-admin` is built, the others come with their callers.
- 2026-11-10: `model-gateway-auditor` has its token from `bootstrap`, and the auditor joins `audit-model-gateway` (`internal`, shared with the gateway alone), so the gateway is the first caller (plan step 3.1, [0054](0054-model-gateway.md)). `server-auditor` and `audit-server` still come with the server's switch (#58).
- 2026-11-11: the server is a caller (plan step 3.1, second half): `server-auditor` appends its gate spans, sealed payloads and the finish of the turn span the gateway opened, over the network `audit-server` (`internal`, the auditor and the server alone); a temporary `server-auditor-read` link reads traces, a trace's spans, a span's sealed payloads, stats and token counts for the Audit pages, which the server prices, and nothing else (no admin actions, prunes or retention). The stats gain `requests_with_tokens` on each group and `by_day_provider` (tokens for each day and provider, no cost), so whoever holds the prices can price each day exactly. The server's own `audit.db` is not opened any more (#59).
- 2026-11-11: the admin reads the store (plan step 2.5): the Audit pages ask `admin-auditor-read` over `audit-admin` for traces, spans, sealed payloads and stats, and the admin applies its prices, so the auditor still holds none. The admin also sets the retention (`PUT /retention`) from its own setting, every 30 seconds and after a change. The temporary `server-auditor-read` link is removed, with its route set (#60).
- 2026-11-11: serves the hash chain to `admin-auditor-read` (plan step 2.6, [0052](0052-askbar-audit-crate.md)): `GET /chain/head` (the newest entry's number and hash), `GET /chain/entries/{id}` (one entry's hash) and `GET /chain/verify` (a full check of every link and record, at most 20 problems listed). No other link may call them (#61).
- 2026-11-11: admin action records (plan step 2.7): an admin action has outcome `requested` (written before the change, so a record that cannot be written refuses it), then `done`, `refused` or `failed`, which name the request in `detail`. The service links (`server-auditor`, `model-gateway-auditor`) gain `POST /config-applied`, stored as an admin action record with the caller from the token, category `config`, action `applied` and the version in `detail`, which is how a service records the configuration it applied ([0028](0028-service-boundaries-and-config-delivery.md) rule 5) (#62).
- 2026-11-11: Caddy's access logs (plan step 2.9, design; built in the same plan step): a second listener, `ingest`, on `auditor-ingest-host:8086` on a new `internal` network `audit-caddy` (Caddy and the auditor alone, no route out). It takes newline-delimited JSON from Caddy's `net` log writer, which has no token, and answers nothing. It keeps only the time, method, host, path **without its query**, status, bytes, duration, client address, protocol and a cut user agent, never a header or a query string, and it refuses a line over 16 KiB. Each line is an append-only `access_logs` row with an entry in the hash chain, pruned with the other records by retention (nothing younger than 7 days). It is best effort: a full queue drops lines and the status says how many. `admin-auditor-read` gets `GET /access-logs`. To keep Caddy from reaching the token API on `audit-caddy`, the API listener no longer binds every interface: it binds one address for each of its token networks (`auditor-admin-host`, `auditor-server-host`, `auditor-model-gateway-host`, port 8083), as the gateway's listeners do. The list under Listener above is superseded by this (#64).
- 2026-11-11: a read link for the gateway, `model-gateway-auditor-read` (`gwrd_`, caller `model-gateway-read`): `GET /token-counts` and `GET /status` and nothing else, so the gateway can hold the budget against what was used, since it has no data volume of its own (plan step 3.2, owner's choice of 2026-11-11). It appends nothing and reads no trace, content or admin record (#66).
- 2026-11-11: body brought up to date with the code on `main` (context, API by link, prune record, networks, listeners, links); the 'Changes elsewhere when built' bullet is gone because those changes are built. No decision changed (#70).
- 2026-11-11: design for plan steps 4.1, 4.2, 6.2 and 6.3 (#70, owner's decisions of 2026-11-11): the `alerter-auditor` link (`POST /hello`, `/config-applied`, `/alert-deliveries`, `GET /status`) and the `auditor-alerter` link, the network `audit-alerter` and its alias `auditor-alerter-host`, the rules engine and the silence, restart-loop and disk rules, the time of the last request on each link, `GET /alert-deliveries`, `PUT /backup-key` and `GET /export` on `admin-auditor-read`, and `admin-auditor-restore` with `POST /restore` into an empty store only. The code PRs add the 'built' lines.
- 2026-11-11: built the auditor's part of alert records (plan step 4.1, PR T, #72). Built so: `alerter-auditor` is in `LINKS` with `POST /hello`, `/config-applied` and `/alert-deliveries` and `GET /status` and nothing else, so every other route is `403` for it; its token hash and network come with the alerter's stack PR, until then the link accepts nothing. `POST /alert-deliveries` takes a kind (lower case, digits and `_`, at most 64), a channel (`ntfy` or `email`), an outcome (`sent`, `failed` or `suppressed`) and 0 to 1,000 attempts, refuses any other field, and stores the caller from the token; the same id twice is one record. `admin-auditor-read` has `GET /alert-deliveries`. `GET /status` carries `last_requests`: when each link last made a request that passed the token check, by caller, in memory only (D2); the silence rule that reads it comes in plan step 4.2.
- 2026-11-11: built the network `audit-alerter` (plan step 4.1, PR U, #74): `internal`, the auditor and the alerter alone; the auditor joins it with the alias `auditor-alerter-host` and binds its token API there (`ASKBAR_AUDIT_API_ADDRS` gains `auditor-alerter-host:8083`), so `alerter-auditor` is live and the alerter's token reaches only its four routes. The auditor's link to send events to the alerter (`auditor-alerter`) has its token from `bootstrap`; the rules that send them are plan step 4.2.
- 2026-11-12: the alert rules are built (plan step 4.2, PR X). Built so: `rules.rs` is fed after each committed append, after each request on a link and every 30 seconds, takes the time as a number, and sends through an `Alerts` trait (the `auditor-alerter` client in the service; `ASKBAR_ALERTER_URL`, default `http://alerter-auditor-host:8085`, which the alias on `audit-alerter` already gives). `unsealed` is a `sealed_content`/`unseal` record that ended `done`; `unseal_timeouts_changed` is a settings save whose `unseal_idle_minutes` or `unseal_total_minutes` differ from the last recorded (the defaults before any; read from the records at start, so a restart is not a change); `backup_key_changed` is `secret`/`backup_key_setup` or `backup_key_rotate` (names to be checked when the admin builds those routes in phase 6); `retention_changed` carries the new days and only a change counts. `audit_disk_low` below 1 GiB (re-armed above 1.25 GiB) and `audit_disk_critical` below 256 MiB (re-armed above 320 MiB), each with the megabytes left. `component_silent` for the server, `model-gateway`, alerter and `admin-read` links after five minutes of nothing (counted from the auditor's start for one never heard); the admin's append link and the gateway's read link are quiet by nature and left out. **Decision to check: `caddy_silent`** is a reply (`chat` turn) that began and, fifteen minutes later, no access line has come since, not "a turn in the last 15 minutes and no line in the last 15 minutes", which would raise it for the first reply after any quiet quarter hour; it applies only where the auditor takes Caddy's lines. `restart_loop` for the admin is three boot-ID changes in its hellos within ten minutes (the admin now sends `POST /hello` at start; the rule is `askbar_service::restart_loop`, shared with the admin's own count for the services it polls). Tested with a fake clock and a collector (`rules.rs`), against a real auditor (`tests/alerts.rs`) and, in `check-isolation.sh` section 3d7, an unseal reaching a stand-in ntfy server with no passphrase in it and a gateway stopped for six minutes raising `component_silent` (CI only: no Docker where this was written).
- 2026-11-12: built `PUT /backup-key` and `GET /export` on `admin-auditor-read` (plan step 6.2, PR AL; [0013](0013-backups.md)). Built so: the key is a setting of the store (`backup_public_key`); `Recorder::snapshot_to` is a `VACUUM INTO` from a read-only connection; the export is a streamed age file of a tar holding `audit.db`, made in `.export-<id>/` on the audit volume, one at a time, refused below the store's size plus `min_free_bytes` (`audit_backlog`). It holds sealed content only. The restore link (`admin-auditor-restore`) is PR AN.
