# 0055. The `alerter` component

Status: Accepted
Date: 2026-11-11
Kind: component
Source: plan steps 4.1 to 4.3 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07); owner's request of 2026-11-11 to submit this record after the Caddy access-log step

## Context

[0006](0006-v1-outbound-access-and-alerts.md) puts alert delivery in a small `alerter` so the auditor stays offline, and gives it direct outbound access in v1. Many records already say "sends an alert": an unseal and a change of its timeouts ([0011](0011-sealed-content-access.md) rule 12), a rolled-back or edited audit chain ([0010](0010-auditor.md) rule 7), a rejected or repeatedly invalid token ([0024](0024-rejected-token-alert.md)), a restart loop ([0005](0005-no-controller-and-self-restart.md)), a failed or missed backup and a backup key change ([0013](0013-backups.md)), a new release ([0017](0017-dependencies-and-patches.md)) and the early warnings of [0016](0016-risk-register.md). Until the alerter exists each of those is a log line and a banner in the admin (plan steps 2.2, 2.6 and 2.8). [architecture.md](../architecture.md) already shows the component, its API path (`/api/alerter/v1`, [0022](0022-api-path-conventions.md)) and its links. [0051](0051-auditor.md) reserves the network `audit-alerter` for it. This record describes the container that plan step 4.1 builds, and that steps 4.2 (the rules) and 4.3 (release checks) extend by amendment.

## Decision

- **What it is:** `services/alerter`, image `askbar-alerter`, serving `/api/alerter/v1`. It links `askbar-service` ([0036](0036-askbar-service-crate.md)) and `askbar-proto` ([0045](0045-askbar-proto-crate.md)), and reaches the auditor over HTTP, so it links no store code. It adds no third-party dependency beyond an SMTP client (`lettre`, added with the email step after `cargo deny` passes); ntfy is an HTTPS `POST` with the HTTP client the workspace already has.
- **What it does:** it delivers alerts and checks for new releases, and nothing else. It decides no rule. It never reads audit content or the audit store.
- **Who sends it an event, and what an event is:**
  - **the auditor**, which sees every record, evaluates the rules that need records and sends `unsealed`, `unseal_timeouts_changed`, `retention_changed`, `backup_key_changed`, `audit_disk_low`, `audit_disk_critical`, `component_silent`, `caddy_silent` and, for the admin (from the admin's hellos), `restart_loop`;
  - **the admin** sends the events only it knows: `chain_mismatch`, `rejected_token`, `restart_loop` for the services it polls, `budget_threshold`, `backup_failed`, `backup_missed` and `restore_performed`;
  - **the alerter itself** raises `release_available` and the test alert.
  - The admin-claim mismatch alerts of [0010](0010-auditor.md) stay deferred (phase 8); they are not `chain_mismatch`. There is no `kill_switch_changed`.
  - An event is `{kind, severity, at, fields}`: a **kind from a fixed list**, a severity (`info`, `warning` or `critical`), a time, and **typed fields only**: `link`, `service`, `period`, `percent`, `count`, `minutes`, `free_mb` and `version`. **There is no free text in an event** and no dedupe key: the alerter derives the key from the kind and the typed fields. It turns a kind into one fixed sentence from a template it owns, so a compromised sender cannot make it say anything else, and no alert ever holds a token, a secret, a prompt or a reply ([0024](0024-rejected-token-alert.md)). The list of kinds is in `docs/openapi/alerter.v1.yaml`; each link may send only its own kinds, and an unknown kind or field is refused. The rules that make the auditor's events are in [0051](0051-auditor.md).
  - Severity maps to the ntfy priority: `info` 3, `warning` 4, `critical` 5.
- **Delivery:** ntfy first (a server, a topic and an optional access token; an HTTPS `POST`), then, from plan step 4 in its own pull request, email through the `lettre` crate (SMTP host, port, TLS mode, user, password, from and to), one or both. The admin sends a test alert (`POST /test`) and shows the result, so the owner learns that delivery is broken when they set it up, not when it matters.
- **Repeat limiting** ([0024](0024-rejected-token-alert.md) consequences): one delivery for each dedupe key in a window of 60 minutes, with the count of suppressed repeats added to the next one, and no more than 20 deliveries an hour in all; when that cap is hit it sends one summary and nothing else until the hour ends. The ledger is a JSON file on its data volume, written atomically, so a restart does not start the count again; a corrupt ledger starts empty. These are starting limits, tuned when the rules of step 4.2 are built.
- **Delivery that fails:** it retries with backoff from a bounded queue in memory (200 events; the oldest is dropped and counted). Each alert's outcome (sent, failed, suppressed) is appended to the auditor with its own append token, as a record with no content. `GET /status` carries the time of the last good and last failed delivery, and the admin shows a banner when delivery has not worked for a while. Nothing outside the host notices if the alerter itself is down; that dead-man monitor is out of v1 ([0006](0006-v1-outbound-access-and-alerts.md), [0016](0016-risk-register.md)), and the admin's banner is the only signal.
- **What it holds:**
  - **A data volume, `alerter-data`:** the repeat ledger, the last release check and the last delivery results. No secret and no content.
  - **Delivery settings and their secrets** (the ntfy topic and token, the SMTP password) are **not on its volume**. The admin keeps them encrypted with its own key ([0040](0040-admin.md)) and pushes them in `PUT /config`, as it does the provider keys; the alerter holds them in memory only, answers `503 not_configured` until they arrive and asks again on a new boot ID ([0028](0028-service-boundaries-and-config-delivery.md) rules 3 to 5). [architecture.md](../architecture.md) lists "`alerter` state and delivery settings" on its volume, which this amends.
  - **Credentials:** its own volume, `alerter-token`, written by `bootstrap` ([0049](0049-bootstrap.md)): the hashes it accepts and the token it sends the auditor.
- **Its API, by link** (each token hash maps to one caller and its routes; anything else is `403`):
  - `auditor-alerter` (the auditor calls it): `POST /events` with the auditor's kinds;
  - `admin-alerter` (the admin calls it): `POST /events` with the admin's kinds, `PUT /config`, `GET /status`, `POST /test`, `POST /restart`, `GET /releases`, and, for backups ([0013](0013-backups.md)), `POST /exports` (the ledger and the release result, encrypted to the backup public key it was pushed in `PUT /config`);
  - `alerter-auditor` (it calls the auditor): the delivery records, `POST /alert-deliveries`, and `POST /hello`, `POST /config-applied` and `GET /status`, on the routes 0051 gives its link.
  - Prefixes are `audalr_`, `admalr_` and `alraud_`.
- **Releases:** once a day it asks `https://api.github.com/repos/Thewuuryar/askbar/releases/latest` (the repository is public) for the newest version, sending no identifier of the install and a fixed `User-Agent`, and keeps the answer for the admin to read ([0017](0017-dependencies-and-patches.md)). A new release is a `release_available` alert. Until 1.0 it is tested only against a mock.
- **Networks and outbound** ([0006](0006-v1-outbound-access-and-alerts.md)): direct outbound in v1, no egress proxy. One network for each link, shared with the alerter alone, none pinned to a subnet:
  - `audit-alerter` and `alerter-admin`, `internal`;
  - **`alerter-outbound`, not `internal`:** the one network the alerter shares with no one. It is not `outbound` ([0054](0054-model-gateway.md)): the gateway and the mock LLM sit there, and a peer on a network with a route out should have no other component to reach.
- **Listeners:** two, one on each caller's network, each bound to the address its alias resolves to (`alerter-auditor-host`, `alerter-admin-host`) and never a wildcard, because the alerter also sits on a network with a route out.
- **Who it runs as:** uid 10005 ([0003](0003-v1-service-layout.md)), from its first instruction.
- **Hardening:** [0032](0032-container-hardening.md)'s defaults with no exception: read-only root, no capabilities, a private `/tmp`, memory 128 MB. `check-isolation.sh` checks:
  - the defaults, `CapEff` 0 and uid 10005;
  - its networks are exactly those above and only `alerter-outbound` has a route out;
  - it mounts `alerter-data` and `alerter-token` (read-only) and nothing else, and nobody else mounts them;
  - the server, the auditor and the gateway still have no route out and share no network with it but the auditor's;
  - a marker delivery secret pushed to the alerter never appears in its logs, its filesystem, any response, the auditor's store or any alert it sends.
- **Changes elsewhere when built (plan step 4.1):**
  - **`bootstrap`** ([0049](0049-bootstrap.md), amended): the three links, the uid and the volume `alerter-token`.
  - **The auditor** ([0051](0051-auditor.md), amended): the network `audit-alerter`, its address for it (the token API binds one address for each network), the link to send events, and the delivery records. The rules that make events are step 4.2.
  - **The admin** ([0040](0040-admin.md), amended): a Settings section for delivery, the test alert, the secrets kept encrypted and pushed, the banner for failing delivery, the services list and its restart, and the events it sends.
  - **Compose, the guide and the plan:** the service, three networks and two volumes.

## Alternatives considered

- **The auditor delivering alerts itself:** rejected in [0006](0006-v1-outbound-access-and-alerts.md); it must stay offline.
- **The admin delivering alerts itself:** rejected. The admin is a likely target and would be silencing its own alarms, and it has no route out.
- **The alerter reading the auditor to find its own events:** rejected. It would need a read token over everything in the store. The auditor already holds the records and pushes a narrow event.
- **Events with free text from the sender:** rejected, because a compromised sender could then use the alerter to carry anything out, and a mistake could put a secret in a message.
- **Delivery secrets on the alerter's volume:** rejected for the reason [0028](0028-service-boundaries-and-config-delivery.md) gives against secrets in each service: a stolen volume would hold them. The cost is one more dependency on the admin after a restart, which already pushes everything.
- **Sharing `outbound` with the gateway:** rejected, for the reason above.
- **A dead-man monitor in v1:** out of scope ([0006](0006-v1-outbound-access-and-alerts.md)).

## Consequences

- A compromised alerter has direct outbound access until the egress proxy ships ([0006](0006-v1-outbound-access-and-alerts.md), [0016](0016-risk-register.md)). It holds only the delivery secrets in memory and sees event metadata, never content, so what it can send out is little, and it can drop or delay alerts.
- A compromised auditor or admin can send false events of its own kinds or leave events out; the other's independent records and the admin's banners are what notice that.
- If delivery is not set up or fails, alerts stay what they are today: a log line and a banner in the admin. The banner says so.
- Nothing outside the host notices a stopped alerter ([0016](0016-risk-register.md)).
- Backups ([0013](0013-backups.md)) export the ledger and release result only; the delivery settings come back from the admin's own backup. The restore of an alerter's volume alone brings back no secret.
- Plan step 4.1 adds three networks and two volumes: the guide's note on Docker's address pools applies.

## Amendments

- 2026-11-11: the owner's decisions for phase 4 (#70): who sends each alert kind and the kinds themselves (including `restore_performed`); the event shape `{kind, severity, at, fields}` with typed fields only; severities `info`, `warning` and `critical` mapped to ntfy priorities 3, 4 and 5; **a decision change:** events no longer carry a dedupe key, the alerter derives it from the kind and the typed fields; `POST /exports` on `admin-alerter` and `POST /alert-deliveries` on `alerter-auditor`; ntfy first, then email through `lettre`; the ledger is a JSON file written atomically; the release check polls GitHub's latest-release API once a day. Design for plan steps 4.1 to 4.3; the code PRs add the 'built' lines.
