# 0009. The audit store lives in its own container

Status: Proposed (the code on `main` keeps `audit.db` in the server; see "When implemented")
Date: 2026-11-04

## Context

Goal from the goals document: "Log everything agents do, so you can investigate later." The owner's priority is a compromised server.

What `main` does today, checked in the code:

- `audit.db` is a SQLite file on the `audit-data` volume, which the server mounts read-write. Append-only is enforced by triggers inside the file: nothing younger than 7 days can be deleted, and a finished span cannot be changed. Code with access to the file can drop a trigger, replace the file, or edit sealed payloads (the `payloads` table has no update trigger). There is no hash chain.
- Events go into a bounded in-memory queue. A failed write logs a warning and the event is lost. Only a full queue refuses new replies (`audit_backlog`). A reply can start before its span is on disk.
- The admin reads the store through four server API routes under `/api/server/v1/audit/`. The record of who opened sealed content (`audit_access`) is already in the admin's database, outside the server.

So a compromised server could change or delete earlier records, which would make investigating the compromise unreliable.

## Decision

1. **A new component, the auditor** (container `askbar-auditor`, its own uid and volume, no route out). Only the auditor holds the audit store. The server mounts none of it.
2. **Two listeners on two internal networks.** `audit-in` joins the server and the auditor and takes records. `audit-read` joins the admin and the auditor and serves reads and the retention setting. The server has no route to the read listener. The API is `/api/audit/v1` with its own OpenAPI file ([0002](0002-one-name-per-role-one-host.md)).
3. **Append only, enforced by the API, not by triggers.** The server's credential can start a span, finish it once, and add sealed payloads. It cannot read, update or delete. The only deletion is the auditor's own retention run, at the number of days the admin sets, never under 7. The admin's credential can read and set retention, nothing else.
4. **A hash chain.** The auditor numbers every record and chains each to the one before. The admin stores the latest chain head in its own database at intervals and checks it against the auditor. A rollback, deletion or edit shows in the Audit tab.
5. **Content stays sealed** ([0004](0004-sealed-audit-content.md)). The server seals before sending; the auditor holds only ciphertext and metadata, and no key.
6. **Fail closed.** The server must get the auditor's acknowledgment of a span's start before it calls a provider. If the auditor is unreachable or behind, new replies are refused with the existing `audit_backlog` (503, with its retry hint). Finish and payload records are queued and retried as today. This is a change from `main`.
7. **Interrupted spans.** On connecting, the server announces a new boot id. The auditor ends that server's earlier unfinished spans as interrupted. The sweep moves from the server to the auditor.
8. **Trust by claim on first contact**, as in [0003](0003-admin-and-server-isolation.md), on each listener. The auditor stores a hash of the server's token and a hash of the admin's token, each claimed once, from that component's fixed address.

## Alternatives considered

- **The admin pulls records into its own database.** Keeps the server from calling anyone, but leaves a window of unpulled records, cannot enforce "recorded before the call," and makes the internet-facing admin hold more.
- **The server pushes to the admin.** Rejected: the server must never call the admin ([0003](0003-admin-and-server-isolation.md)).
- **Keep the store in the server with stronger triggers.** Rejected: anything with write access to the file can remove triggers.
- **An off-site sink only.** Strongest against a compromised host, but needs hardware and a decision on where. Listed for later in [plan-container-hardening.md](../plan-container-hardening.md).

## Consequences

- A compromised server cannot alter or remove records it has already sent. A compromised admin cannot alter any record, and cannot remove one younger than 7 days.
- This protects history up to the moment of compromise. Afterwards the server can still send false records, leave some out, or flood the auditor. Records the server does not control are needed to check its account: Caddy's access log for what came in, and an egress proxy's log for what went out. Both are proposed in the hardening plan, with the auditor as their destination.
- If the auditor is down, chat stops. The auditor needs its own backup, by copying `auditor-data`.
- The server API loses `/api/server/v1/audit/*`. Architecture says removals within `v1` need a `v2` beside it for one release. This API is internal and ships with the admin, so the owner should decide: either a `v2`, or a recorded exception.

## When implemented

Add the service, volume and two networks to `compose.yaml`; move the store, its writer and the sweep out of `askbar-server`; add `docs/openapi/audit.v1.yaml` and its tests; point the admin's audit proxy and chain check at the auditor. Update `scripts/check-isolation.sh` (the "3b. The audit store" section: only the auditor mounts `audit-data`, the server and admin do not) and prove the server cannot reach the read listener. Update `SECURITY.md`, `architecture.md` (picture, routes, volumes, networks), `docs/style/logging-and-audit.md` ("Only the server mounts `audit.db`") and the README's Audit tab text.
