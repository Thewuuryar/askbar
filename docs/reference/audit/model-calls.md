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

When redaction is on and a request holds personal data, each attempt has a `redact` step (kind `step`, written by `model-gateway`); a request with nothing found has none. Its unsealed `attrs.pii` holds only numbers and names: `counts` (found, by type, for example `EMAIL: 2`), `actions` (what applies to each type: `pseudonymize`, `block` or `allow`), `mode` (`detect_only` or `enforce`) and, in `enforce` mode only, `replaced` (how many values were swapped for placeholders) and `restored` (how many placeholders were put back in the reply). In `detect_only` mode `replaced` and `restored` are absent. No value, snippet or offset is ever in them. You see them in this page: choose a reply, then the `redact` row of its trace, whose details list the attributes. The text the provider received is the sealed `provider_request` of the attempt (redacted in `enforce` mode, as typed otherwise).

Target, once context checks are built ([plan](../../plan-canaries.md); the ADR amendments are proposed): the attempt's unsealed `attrs.context` holds `estimated_in` (input tokens Askbar estimates it sent), `reported_in` (input tokens the provider reports; absent when the count is itself an estimate), `truncated` (true when the reported count is below 0.6 of the estimate, a sign that the provider dropped part of the context) and `stop` (the provider's raw stop reason, for example `length` or `max_tokens`, or null). When a provider has a context limit and the request is at or above 70% of it, the attempt also has a `canary` step (kind `step`, written by `model-gateway`) with `attrs.canary = {planted, outcome, instruction_version}`: `planted` is a boolean, `outcome` is `ok`, `front_lost` (the start of the context was dropped), `absent` (the model ignored the instruction, so the check is inconclusive) or `skipped` (the output limit cut the reply first), and `instruction_version` is a number. No marker text is ever in them. You see them in this page: choose a reply, then the `canary` row of its trace. A canary is a tripwire, not proof that the model used the context.

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

## Research replies

A research report has the trace of any reply, plus these records. The question, sub-questions, page text, summaries and draft are sealed payloads as for any turn, and none of them is in an attribute or a log.

| Record | Written by | Holds |
| --- | --- | --- |
| `turn` `chat`, one per model call | `model-gateway` | `role`: `research.researcher` (summarises one sub-question's pages), `research.writer` (splits the question, then writes the draft) or `judge.review` (the verdict, and the reason when refused), with the provider and model. |
| `gate` `research_approval`, one per tool call | `model-gateway` | The same as a quick-search approval: a refused call is a blocked span with its reason and counts. |
| `tool` `search` and `tool` `fetch` | `mcp-gateway` | As for quick search. |
| `gate` `research_refused`, when no research starts | `server` | `reason`: `disabled`, `unavailable`, `not_permitted`, `busy` (a report is already running) or `unpinned`. |

The task's steps (role, status and a code word, never text) are kept in the conversation's space file, not in the audit trail, and are deleted with the conversation. The rules are in the [guide](../../guide.md#research-reports).
