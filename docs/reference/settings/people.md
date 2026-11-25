---
id: settings/people
title: People & clients
summary: Who can use Askbar, and the phones and computers paired to each of them.
keywords: [users, clients, pairing, people, phones, computers]
settings: [people]
---

# People & clients

Who can use Askbar, and the phones and computers paired to them. A user is a person the admin creates, who owns clients and a personal space; a client is one paired app and its token. The meanings are in the [glossary](../../architecture.md#glossary).

## Users

Who can use Askbar. Pause a person to stop their replies until you resume them.

## Paired clients

The phones and computers paired to a person. Generate a pairing key for a person, then paste it into the Askbar app (Pair with server) to pair it.

## Search and fetch permissions

The built-in tools that let a reply search the web (`search`) and read a page (`fetch`) are held by no one until you allow them (ADR 0019). The settings page for them is planned for [Phase 8](../../plan-tiered-routing/phase-8-admin-ui.md) (P8.6a) and is not built, so for now use the API, all with an admin session or API key:

- `GET /api/admin/v1/mcp/permissions`: the users who have permissions set, keyed by user id.
- `PUT /api/admin/v1/mcp/permissions/{user}`: sets the tools one user may use (`allowed`) and may not use (`denied`), replacing both lists. A tool in neither list is not held; a tool in both is refused. The change is recorded in the auditor, and is refused with 503 if it cannot be.
- `DELETE /api/admin/v1/mcp/permissions/{user}`: removes every permission of that user.

The limits are fixed defaults that the admin pushes to the gateway and the server; no page sets them yet. A search returns up to 8 results, and a page read is capped at 1 MiB, times out after 20 seconds and follows up to 3 redirects. The quick-search limits are in the [guide](../../guide.md#searching-the-web). Refused requests to the search engines and to sites are listed on [Services](services.md#blocked-outbound-requests).
