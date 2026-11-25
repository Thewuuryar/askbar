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

The built-in tools that let a reply search the web (`search`) and read a page (`fetch`) are held by no one until you allow them (ADR 0019). Set them for each person on the [MCP](mcp.md#tool-permissions) page, or through the API, all with an admin session or API key:

- `GET /api/admin/v1/mcp/permissions`: the users who have permissions set, keyed by user id.
- `PUT /api/admin/v1/mcp/permissions/{user}`: sets the tools one user may use (`allowed`) and may not use (`denied`), replacing both lists. A tool in neither list is not held; a tool in both is refused. The change is recorded in the auditor, and is refused with 503 if it cannot be.
- `DELETE /api/admin/v1/mcp/permissions/{user}`: removes every permission of that user.

The limits are on [Search](search.md#mcp-limits) and [Search](search.md#quick-search). Refused requests to the search engines and to sites are listed on [Egress](egress.md#blocked-outbound-requests).
