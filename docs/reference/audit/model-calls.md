---
id: audit/model-calls
title: Model calls
summary: What the server recorded about each reply, the figures for a range, and the trace of a single reply.
keywords: [model calls, audit, trace, replies, tokens, latency, errors, status, provider, sealed]
settings: [audit/calls]
---

# Model calls

What the server recorded about each reply: when, for whom, through which provider, how long it took and how it ended. The strip at the top shows the replies, errors, tokens and latency (median and 95th percentile) for the range you choose. Below it, the list can be filtered by range, status and provider, newest first; choosing a time opens the trace of that reply in the side panel, with a timeline of its steps and its details. The list holds metadata only: message content stays sealed, and opening it asks for the audit passphrase and is recorded (see [Sealed access](sealed-access.md)). Filtering by user or model, searching and exporting are shown greyed because the audit log cannot do them yet. How the records are kept is explained in [The Audit page](../../guide.md#the-audit-page).

Target, not yet built ([plan](../../plan-audit-viewer.md) AV2.1 to AV2.3; the ADR amendments are proposed): a reply's trace also holds `step` spans, written by `model-gateway` only, each a child of an attempt, with the decision and counts and never values. Sealed content has fixed roles: `prompt` (server, only the new message as typed), `request` (server, as typed before redaction, with the history), `provider_request` (`model-gateway`, what the provider received after redaction and canaries, sealed on the attempt) and `reply` (server, what the user saw). Records from before this have `request` and `reply` only.

## Quick-search replies

A reply that searched the web has the trace of any reply, plus these records. None of them holds a query, a page or a page's text in its attributes or logs; the message, the queries and the answer are sealed as for any turn.

| Record | Written by | Holds |
| --- | --- | --- |
| `turn` `chat`, one per model call | `model-gateway` | `role`: `search.query` (proposes the queries) or `search.writer` (writes the answer), with the provider and model. |
| `gate` `quick_search_approval`, one per search or page request | `model-gateway` | `tool`, `action_hash`, `one_time_id` and `expires_at`. A refused request is a blocked span with counts and limits. |
| `tool` `search`, one per search | `mcp-gateway` | `action_hash` and `one_time_id`. |
| `tool` `fetch`, one per redirect hop | `mcp-gateway` | `url` (the full address of that hop) and `hop` (0 for the first). |
| `gate` `quick_search_refused`, when no search is made | `server` | `reason`: `not_permitted` (the user does not hold `search`) or `unavailable` (the search tools cannot be reached). |

The fetched text is not kept in the conversation. The rules are in [Sealed access](sealed-access.md) and the [guide](../../guide.md#searching-the-web).
