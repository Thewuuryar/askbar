# 0009. Spaces and storage

Status: Accepted 
Date: 2026-11-04
Plan decisions: 8, 30

## Context

Users need private data and optional shared groups. A query bug in one user's or group's data must not expose another's.

## Decision

Each personal space and shared space has its own SQLite file, with membership in the control database. The admin can create as many shared spaces as they want, or none. Memory is preferred in v1, but can be deferred.

| File | Holds | Who can open it |
| --- | --- | --- |
| Control database | Users, clients, permissions, budgets (a copy; `model-gateway` holds the authoritative one, [0014](0014-model-gateway-and-providers.md)), pairing hashes, spaces, memberships | The server, for control checks |
| Personal space, one per user | Private conversations, history, memory, settings, search | That user |
| Shared space, any number | Shared conversations, history, memory, search | Its members |

**Mechanism**

1. The admin can create, rename, and delete shared spaces, and add or remove members.
2. The server checks membership in the control database, then gives the handler only that space's store.
3. `/api/client/v1` gains endpoints that list a user's spaces and start a conversation in one.
4. A space's file is created with the space. Migrations run on every file, and every file is included in backups.
5. The audit record for each reply includes the space ID.

**Rules**

- Each conversation belongs to one space, and the model uses memory from that space only.
- Moving content between spaces is an explicit copy, never automatic.
- Each memory entry records who or what created it.
- New members can see existing history, and the admin is told this before adding someone.
- Removed members lose access, and their shared content stays in the space.

## Alternatives considered

- **One database with a group ID on every row:** rejected, because a missing filter could leak data between groups.
- **PostgreSQL with row-level security:** deferred. Consider it only for heavy simultaneous writes or access from multiple hosts.
- **One PostgreSQL schema or database per group:** rejected, because each migration must run per group and it adds a database service.

## Consequences

Search across spaces must query each file and merge results. SQLite allows one writer per file and needs one host. Storage stays behind a per-space interface to limit any later rewrite. A compromised server container can still open all space files ([0016](0016-risk-register.md)).
