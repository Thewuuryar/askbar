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

What is missing:
- a conversation-level view and one unseal action for it;
- records of the gateway's own steps;
- a request payload that holds only the new message, not the whole history again.

Sealing stays as it is ([ADR 0011](adr/0011-sealed-content-access.md)). The owner considered storing content in plaintext behind a hidden field; that is ADR 0011's rejected "plaintext behind an admin gate" alternative, and this plan does not reopen it. Sealing protects confidentiality only. Integrity comes from the append-only links, the hash chain and the admin's chain check, which this plan doesn't change.

## Design

### A. Unseal a whole conversation as one action

- **Conversations list (no unseal needed):** grouped by `conversation_id`, showing user name, space, client, first and last time, turns, models, tokens, cost, and flags (refusals, retries, truncation, PII blocks, canary findings). User ids are matched to names from `admin.db` when the page is shown, so names never enter the audit store.
- **Open conversation:**
  - **Before it opens:** asks for an open unseal session, then requires a reason the owner types; the reason is stored sealed.
  - **What it fetches:** every payload of every span in the conversation, in one request through the existing audit read link.
  - **What it shows:** one nested timeline.
- **Record and alert:** one sealed-access record listing every span id opened, plus one alert ("opened Sam's conversation of 21 Nov, 6 turns"), replacing one record per item for this view.
- **Limits:** one conversation per action; results are never cached on disk; responses carry `Cache-Control: no-store`; admin API keys still can't open sealed content.
- **Scope:** a conversation is a natural boundary, so ADR 0011's "no bulk export" rule gains one exception (a single conversation) rather than being dropped.

### B. Record the gateway's steps

A new span kind `step`, one per stage, in order. Gateway steps are children of an attempt: `route`, `redact`, `canary`, `provider`, `restore`. Server steps are children of the turn, for flows the server runs: quick search (`search_plan`, `search`, `fetch`, `answer`) and, later, research reports. Each step's unsealed attributes hold the decision and counts, never values. Examples:
- `redact`: `{counts: {EMAIL: 1, PERSON: 2}, actions: {...}}`;
- `canary`: `{outcome: "front_lost"}`;
- `route`: the decision record from tiered routing, `{decision_id, tier_final, requested, model, reasons}`.

Sealed payloads, with fixed roles:

| Role | Written by | Holds |
| --- | --- | --- |
| `prompt` | server | the new user message as typed, plus a hash of the history sent with it |
| `request` | server | unchanged, for records before this plan; new turns write `prompt` instead |
| `provider_request` | model-gateway | the full request the provider received, after redaction and canaries |
| `provider_reply` | model-gateway | the raw provider reply before restore and stripping, only when it differs |
| `reply` | server | the reply the user saw |

Writing only the new message (`prompt`) cuts storage from quadratic to linear in conversation length, and the timeline no longer needs a diff to show what each turn added. The full history is still recoverable from `provider_request`.

### C. A separate audit key for each user

Today one private key opens everything. Instead:

- At user creation, the admin makes a random X25519 key pair for that user, wraps the private half with a sealed box to the master audit public key, and stores the wrapped key in `admin.db`. No passphrase is needed for this, so users can be created at any time.
- The server and gateway receive each user's public key in their config and seal that user's payloads to it.
- To open a conversation, the admin uses the master secret (from the passphrase, as today) to unwrap only that user's key. The session keeps only the user keys it has opened; the master secret is wiped after each unwrap.
- Passphrase rotation rewraps the user keys; old records keep their key id, as today.
- **Effect:** a compromised admin during a session reaches the users opened in that session, not everyone. Records and alerts name the user whose key was used.

### D. Decrypt in the browser (optional, later)

The admin sends only ciphertext; the browser derives the key from the passphrase, or from a passkey through the WebAuthn PRF extension, and decrypts locally. The admin process then never holds keys or plaintext, so reading its memory or database gains nothing. A compromised admin can still serve changed JavaScript, so this helps against an attacker who can only read, not one who can change code. The passkey also makes unsealing quicker. Deferred until a passkey is set up.

### Search inside a session (optional, Phase 4)

- **Scope:** a search covers one user or space and a time range. Results are capped at 50, decrypted one record at a time in memory and wiped after matching. Nothing is indexed on disk.
- **Record:** each search records its scope, the number of records scanned and the ids returned, with the query sealed, and sends an alert.
- **Hardening:** queries are sent by POST, results carry `no-store`, and core dumps are off for the admin container.

## What the view shows

Header: **user name** · space · client · first and last time · turns · tokens in, out and cached · cost · flags.

| # | Time | Prompt (as typed) | Gateway | Model | Reply | In / out / cached | TTFT | Total | Stop | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 19:02:11 | Draft an email to Mrs Lee… | PII: PERSON, EMAIL pseudonymized · route: default | sonnet | Hi Mrs Lee, … | 1.2k / 310 / 0 | 0.6 s | 4.1 s | complete | done |
| 2 | 19:04:40 | Shorter please | PII: none · route: default | sonnet | Hi Mrs Lee, … | 1.6k / 120 / 1.1k | 0.5 s | 1.9 s | complete | done |
| 3 ▸ | 19:09:02 | Summarise this… | PII: 4 · canary: front lost · 1 retry (429) | sonnet | … | 31k / 400 / 1.5k | 2.2 s | 9.8 s | complete | done |

Expanding a row shows:
- the gate checks;
- web searches and fetches (tool spans: query, full URL, status, size), and research or judge steps with their role and model;
- each attempt with its HTTP status and retry reason;
- each step in order with its attributes;
- `prompt` next to `provider_request`, with redacted spans highlighted;
- `provider_reply` next to `reply`;
- the routing decision.

Refused turns appear in place with their gate and reason. Records written before this plan show the old `request` payload and no steps.

## Owner gates

- **Phase 0:** accept the amendments.
  - [ADR 0011](adr/0011-sealed-content-access.md): conversation-scoped opening, the reason field, per-user keys, and the search rules if Phase 4 goes ahead.
  - [ADR 0010](adr/0010-auditor.md), [ADR 0051](adr/0051-auditor.md) and [ADR 0052](adr/0052-askbar-audit-crate.md): the `step` span kind and the payload roles.
  - [ADR 0040](adr/0040-admin.md): the new page and API.
  - [docs/style/logging-and-audit.md](style/logging-and-audit.md): the step attributes.
- **Before Phase 3:** the owner chooses whether to move existing records to per-user keys (it needs the passphrase and rewrites no records: old records keep the master key id) or to start per-user keys for new records only.

## Phases

| Phase | Content | Status |
| --- | --- | --- |
| 0 | ADR amendments | not started |
| 1 | Conversations list and conversation-scoped opening | not started |
| 2 | Step spans and payload roles (AV2.1 to AV2.3 run in track 1, stage W1) | not started |
| 3 | Per-user audit keys | not started |
| 4 | Search inside a session (optional) | not started |
| 5 | Browser decryption with passkey (optional) | not started |

### Phase 0: records

- [ ] **AV0.1** `docs(adr): amend 0011: open a conversation as one action` · sonnet
  Accept: the rule names the one-conversation exception to "no bulk export", the reason field, the single access record and alert.
- [ ] **AV0.2** `docs(adr): amend 0010, 0051 and 0052: step spans and payload roles` · sonnet · runs in track 1 (W0), before AV0.1; no longer after it
  Accept: `step` is a span kind; the five payload roles are listed with their writers; `docs/reference/audit/model-calls.md` matches.
- [ ] **AV0.3** `docs(adr): amend 0011: per-user audit keys` · sonnet · after AV0.1

### Phase 1: conversation view

- [ ] **AV1.1** `feat(auditor): list traces and payloads by conversation` · sonnet
  Files: `services/auditor/src/lib.rs`, `crates/askbar-audit/src/lib.rs`, `docs/openapi/audit.v1.yaml`.
  Do: a read route that filters turn spans by `conversation_id` from attributes (index with a generated column if needed), and a route that returns every payload for a list of span ids. Read link only.
  Accept: the chain and sealing are untouched; a test returns one conversation's spans in order.
- [ ] **AV1.2** `feat(admin): conversations list and conversation timeline` · sonnet · after AV1.1
  Files: `services/admin/src/{audit_read,admin_api,sealed}.rs`, `docs/openapi/admin.v1.yaml`.
  Do: `GET /api/admin/v1/audit/conversations` (no unseal: metadata, names joined from `admin.db`, flags from attributes) and `POST /api/admin/v1/audit/conversations/{id}/open` (unseal session and reason required; one access record listing span ids, written before anything is returned; one alert).
  Accept: without a session the open route refuses with an `ErrorCode`; with one it returns the nested timeline; the access record is refused-closed if it can't be written.
- [ ] **AV1.3** `feat(ui): conversation view` · sonnet · after AV1.2
  Files: `services/admin/src/pages.rs`, a new template under `services/admin/src/ui/templates/`, `app.js`, admin browser test.
  Do: the list and the table above, with expandable rows. Text is written with `textContent` only; the page clears on seal or session timeout.
  Accept: the browser test opens a seeded conversation and sees turns in order, and the sealed content is gone after Seal.

### Phase 2: steps and payload roles

- [ ] **AV2.1** `feat(audit): add the step span kind` · sonnet
  Files: `crates/askbar-proto/src/audit.rs`, `crates/askbar-audit/src/lib.rs` (append a migration for the kind check), `services/auditor/src/lib.rs` (`may_start`: `model-gateway` opens steps under its attempts, and the server opens steps under its own turns).
  Runs in track 1, stage W1.
  Accept: old records verify unchanged; a step under an attempt is stored and chained.
- [ ] **AV2.2** `feat(server): seal the new message as the prompt payload` · haiku · after AV2.1 · runs in track 1, stage W1
  Files: `services/server/src/generation.rs`.
  Do: seal `prompt` with the new message and a SHA-256 of the history sent, instead of `request`.
- [ ] **AV2.3** `feat(model-gateway): record gateway steps and provider payloads` · sonnet · after AV2.1 · runs in track 1, stage W1
  Do: open a `step` span for each stage that ran; seal `provider_request` and, when it differs, `provider_reply`. Stages from the canary and PII plans and the routing decision plug in here as they ship.
  Accept: a turn with redaction shows `redact`, `provider` and `restore` steps in order; no values in attributes (a test greps for a planted value).
- [ ] **AV2.4** `feat(ui): show steps and side-by-side payloads` · sonnet · after AV2.3 and AV1.3

### Phase 3: per-user keys

- [ ] **AV3.1** `feat(admin): make a wrapped audit key for each user` · sonnet
  Files: `services/admin/src/{audit_key,db,admin_api}.rs`.
  Do: generate at user creation, wrap to the master public key, store wrapped; rewrap on passphrase rotation; push public keys in the server snapshot and gateway config.
  Accept: no unwrapped user key is ever written; rotation keeps every user's records openable.
- [ ] **AV3.2** `feat(server): seal each user's payloads to their key` · haiku · after AV3.1
- [ ] **AV3.3** `feat(model-gateway): seal each user's payloads to their key` · haiku · after AV3.1
- [ ] **AV3.4** `feat(admin): open conversations with the user's key only` · sonnet · after AV3.2 and AV3.3
  Do: unwrap the user key, wipe the master secret, keep user keys for the session only; name the user in the record and alert.
  Accept: a session that opened user A cannot decrypt user B's payloads without the passphrase again.

### Phase 4: search (optional)

- [ ] **AV4.1** `feat(admin): search one user's or space's sealed content` · sonnet
  Do: the rules under Search inside a session; decrypt one record at a time with `zeroize`; disable core dumps in the admin container.
  Accept: each search writes its record before returning results; more than 50 matches returns the first 50 and a count.

### Phase 5: browser decryption (optional)

- [ ] **AV5.1** Design spike only: WebAuthn PRF support across the owner's browsers, where the key derivation runs, and what the admin still records. Report back before any code.

## Risks

- **Bigger single disclosure:** opening a conversation decrypts more at once than one item. Mitigated by the reason field, the single record and alert, and no caching.
- **Earlier tracks:** step records arrive in track 1, so tracks 1 to 4 already write `step` spans; only records from before track 1 lack them.
- **Migration:** records from before Phase 2 have the whole history in `request`; the view shows them without steps and without the new-message split.
- **Hash chain:** a new span kind and payload roles change what is stored, not how the chain is computed; old records must keep verifying (AV2.1 test).

## Not in scope

- Storing content in plaintext (rejected in ADR 0011).
- Cache-write token counts (owner decision, 2026-11-23).
