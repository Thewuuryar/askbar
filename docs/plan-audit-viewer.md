# Plan: conversation view for sealed audit records

Status: proposed, after 1.0. Track 5, the last, in [plan-priorities.md](plan-priorities.md). By owner decision (2026-11-24), the step records (AV0.2 and AV2.1 to AV2.3) run first, as stage W1 of track 1, so every later track records in this shape; the rest of this plan stays in track 5. Nothing here is decided until the owner accepts the ADR amendments in Phase 0. Rules for agents: [AGENTS.md](../AGENTS.md).

Related plans: [context checks and canaries](plan-canaries.md) and [PII detection and redaction](plan-pii-redaction.md) add the gateway steps this view shows; [tiered routing](plan-tiered-routing.md) adds the routing decision record.

## Summary

The owner wants to open a conversation and read, in order, everything that happened in it:
- who sent each message (by name);
- the prompt as typed;
- what `model-gateway` did to it (PII redaction, canaries, routing);
- what the provider received and returned;
- what the user saw;
- the metrics (tokens, cached tokens, timings, stop reason, cost).

Today the sealed view opens one span's payloads at a time (`GET /api/admin/v1/audit/payloads/{span_id}`), so following a conversation means opening each item by hand.

The data needed for grouping is already stored unsealed:
- turn spans carry `user_id`, `client_id`, and `space_id` and `conversation_id` in their attributes;
- every span of one reply shares a trace id;
- attempts and gate checks point to their parent turn.

The admin already has a trace view: `audit_traces` and `audit_trace` return every span of a reply with parent id, kind, attributes, tokens, TTFT and a sealed payload count.

What is missing:
- a conversation filter and grouping on that view, and one unseal action for a whole conversation;
- records of the gateway's own steps;
- a payload that holds only the new message, because `request` is cut from the front at 256 KiB and a long history can lose the newest message.

Sealing stays as it is ([ADR 0011](adr/0011-sealed-content-access.md)). The owner considered storing content in plaintext behind a hidden field; that is ADR 0011's rejected "plaintext behind an admin gate" alternative, and this plan does not reopen it. Sealing protects confidentiality only. Integrity comes from the append-only links, the hash chain and the admin's chain check, which this plan doesn't change.

## Design

### A. Unseal a whole conversation as one action

Build on the existing trace view.
- **Conversations list (no unseal needed):** traces filtered and grouped by `conversation_id`, showing user name, space, client, first and last time, turns, models, tokens, cost, and flags (refusals, retries, truncation). User ids are matched to names from the `users` table in `admin.db` when the page is shown, so names never enter the audit store.
- **Cost:** `audit_trace` takes cost from the first turn only. It must sum over every turn in a trace, because quick search and research put several model calls in one reply.
- **Open conversation:** one admin route.
  - **Before it opens:** it checks the unseal session.
  - **Access rows:** it writes the existing per-span access rows (`record_audit_access`) before returning any content.
  - **What it fetches:** the admin loops the existing per-span payload route. The auditor gets no new batch route.
  - **Alert:** one alert per open ("opened Sam's conversation of 21 Nov, 6 turns").
- **Limits:** one conversation per action; results are never cached on disk; responses carry `Cache-Control: no-store`; the page writes text with `textContent` only; admin API keys still can't open sealed content.
- **Scope:** a conversation is a natural boundary, so ADR 0011's "no bulk export" rule gains one exception: opening one conversation in one action.

### B. Record the gateway's steps

A new span kind `step`, written by `model-gateway` only, as a child of an attempt. The server never opens steps: the auditor's `may_start` lets `model-gateway` open any kind and the server only `gate` spans. Search and fetch are `tool` spans from the MCP front; quick-search model calls are ordinary turns.

Each step's unsealed attributes hold the decision and counts, never values. Later tracks add their own steps when they ship, for example:
- `redact`: `{counts: {EMAIL: 1, PERSON: 2}, actions: {...}}`;
- `canary`: `{outcome: "front_lost"}`;
- `route`: the decision record from tiered routing, `{decision_id, tier_final, requested, model, reasons}`;
- `restore`: the restored count.

Sealed payloads, with fixed roles:

| Role | Written by | Holds |
| --- | --- | --- |
| `prompt` | server | only the new user message as typed |
| `request` | server | unchanged: the request as typed, before redaction; it still holds the history |
| `provider_request` | model-gateway | the full request the provider received, after redaction and canaries; sealed on the attempt span |
| `reply` | server | the reply the user saw |

`prompt` exists because `request` is capped at 256 KiB from the front, so a long history can lose the newest message. `prompt` always keeps it, and the timeline shows what each turn added without a diff.

## What the view shows

Header: **user name** · space · client · first and last time · turns · tokens in, out and cached · cost · flags.

| # | Time | Prompt (as typed) | Gateway | Model | Reply | In / out / cached | TTFT | Total | Stop | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 19:02:11 | Draft an email to Mrs Lee… | PII: PERSON, EMAIL pseudonymized · route: default | sonnet | Hi Mrs Lee, … | 1.2k / 310 / 0 | 0.6 s | 4.1 s | complete | done |
| 2 | 19:04:40 | Shorter please | PII: none · route: default | sonnet | Hi Mrs Lee, … | 1.6k / 120 / 1.1k | 0.5 s | 1.9 s | complete | done |
| 3 ▸ | 19:09:02 | Summarise this… | PII: 4 · canary: front lost · 1 retry (429) | sonnet | … | 31k / 400 / 1.5k | 2.2 s | 9.8 s | complete | done |

Expanding a row shows:
- the gate checks;
- web searches and fetches (tool spans from the MCP front: query, full URL, status, size), and research or judge model calls with their role;
- each attempt with its HTTP status and retry reason;
- each step in order with its attributes;
- `prompt` next to `provider_request`, with redacted spans highlighted;
- `reply`;
- the routing decision.

Refused turns appear in place with their gate and reason. Records written before this plan show the old `request` payload and no steps.

## Owner gates

- **Phase 0:** accept the amendments.
  - [ADR 0011](adr/0011-sealed-content-access.md): opening one conversation in one action is the one exception to "no bulk export".
  - [ADR 0010](adr/0010-auditor.md), [ADR 0051](adr/0051-auditor.md) and [ADR 0052](adr/0052-askbar-audit-crate.md): the `step` span kind and the payload roles.
  - [ADR 0040](adr/0040-admin.md): the new page and route.
  - [docs/style/logging-and-audit.md](style/logging-and-audit.md): the step attributes.

## Phases

| Phase | Content | Status |
| --- | --- | --- |
| 0 | ADR amendments | not started |
| 1 | Conversation filter, view and conversation-scoped opening | not started |
| 2 | Step spans and payload roles (AV2.1 to AV2.3 run in track 1, stage W1) | not started |

### Phase 0: records

- [ ] **AV0.1** `docs(adr): amend 0011: open a conversation as one action` · sonnet
  Accept: the rule names the one-conversation exception to "no bulk export".
- [ ] **AV0.2** `docs(adr): amend 0010, 0051 and 0052: step spans and payload roles` · sonnet · runs in track 1 (W0), before AV0.1; no longer after it
  Accept: `step` is a span kind written by `model-gateway` only; the payload roles are listed with their writers; `docs/reference/audit/model-calls.md` matches.

### Phase 1: conversation view

- [ ] **AV1.1** `feat(admin): filter traces by conversation` · sonnet
  Files: `services/admin/src/{audit_read,admin_api}.rs`, `services/auditor/src/lib.rs` only if the filter runs there, `docs/openapi/admin.v1.yaml`.
  Do: add a `conversation_id` filter to trace listing (`json_extract` on the turn attributes) and to `AUDIT_QUERY_KEYS`. No new payload batch route: the admin loops the existing per-span route.
  Accept: the chain and sealing are untouched; a test returns one conversation's traces in order.
- [ ] **AV1.2** `feat(admin): conversation list and open conversation` · sonnet · after AV1.1
  Files: `services/admin/src/{audit_read,admin_api,sealed}.rs`, `docs/openapi/admin.v1.yaml`.
  Do:
  - group the trace list by conversation, with user names joined from `users` at read time;
  - fix `audit_trace` to sum cost over every turn in a trace;
  - add one route to open a conversation: it checks the unseal session, writes the per-span access rows before returning content, and sends one alert per open.

  Accept: without a session the open route refuses with an `ErrorCode`; with one it returns the nested timeline; access rows are written first and the open is refused if they can't be written; a trace with two turns shows the summed cost.
- [ ] **AV1.3** `feat(ui): conversation view` · sonnet · after AV1.2
  Files: `services/admin/src/pages.rs`, a new template under `services/admin/src/ui/templates/`, `app.js`, admin browser test.
  Do: the list and the table above, with expandable rows. Responses carry `Cache-Control: no-store`. Text is written with `textContent` only; the page clears on seal or session timeout.
  Accept: the browser test opens a seeded conversation and sees turns in order, and the sealed content is gone after Seal.

### Phase 2: steps and payload roles

- [ ] **AV2.1** `feat(audit): add the step span kind` · sonnet
  Files: `crates/askbar-proto/src/audit.rs`, `crates/askbar-audit/src/lib.rs` (rebuild the spans table with the new kind check, as the v5 migration did, because the CHECK can't be altered in place), `services/auditor/src/lib.rs` (`may_start`: `model-gateway` may open steps under its attempts; the server still opens only `gate` spans).
  Runs in track 1, stage W1.
  Accept: old records verify unchanged; a step under an attempt is stored and chained.
- [ ] **AV2.2** `feat(server): seal the new message as the prompt payload` · haiku · after AV2.1 · runs in track 1, stage W1
  Files: `services/server/src/generation.rs`.
  Do: seal a `prompt` payload with just the new message, next to `request`, which stays as it is (it holds the pre-redaction text). `prompt` is needed because the 256 KiB front cut on `request` can drop the newest message.
  Accept: a test with a history over 256 KiB still has the new message in `prompt`.
- [ ] **AV2.3** `feat(model-gateway): seal provider_request on the attempt` · sonnet · after AV2.1 · runs in track 1, stage W1
  Files: `services/model-gateway/src/lib.rs`.
  Do: seal `provider_request` on the attempt span, where the gateway already seals payloads. Later tracks add their own `step` spans (`route`, `redact`, `canary`, `restore`) when they ship.
  Accept: a step span under an attempt is stored and chained; `provider_request` decrypts in the sealed view; a test greps step attributes for a planted value and finds nothing.
- [ ] **AV2.4** `feat(ui): show steps and side-by-side payloads` · sonnet · after AV2.3 and AV1.3

## Risks

- **Bigger single disclosure:** opening a conversation decrypts more at once than one item. Mitigated by the access rows written first, the single alert, and no caching.
- **Earlier tracks:** step records arrive in track 1, so tracks 1 to 4 already write `step` spans; only records from before track 1 lack them.
- **Migration:** records from before Phase 2 have the whole history in `request` and no `prompt`; the view shows them without steps and without the new-message split.
- **Hash chain:** a new span kind and payload role change what is stored, not how the chain is computed; old records must keep verifying (AV2.1 test).
- **Table rebuild:** AV2.1 rebuilds the spans table; the v5 migration is the precedent.

## Not planned (park)

- Per-user audit keys: one key per user, wrapped to the master key.
- Search inside a session.
- Decrypting in the browser with a passkey.
- Storing content in plaintext (rejected in ADR 0011).
- Cache-write token counts (owner decision, 2026-11-23).
