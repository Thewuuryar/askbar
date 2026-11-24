# Phase 2: shadow (branch `feat/routing-shadow`)

Previous: [Phase 1](phase-1-records.md) · Next: [Phase 3](phase-3-auto.md) · Tracker: [task-tracker.md](../task-tracker.md)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

The decider records its pick while users' chosen models route.

- [ ] **P2.1** `feat(proto): add decider config` · haiku · after Phase 1
  Files: `crates/askbar-proto/src/gateway.rs`.
  Do: `decider {providers, timeout_ms, chat, judge, step_check, research}`, `chat_auto {default_tier, user_ceiling, cutoffs, mode, training_share}`, decider models (`kind: decider`, `host`), key field, `hosted_approved`, `local_to_cloud`, training examples. All defaulted; defaults and bounds for the numbers come from the P1.1a constants (add any missing one there and in `constants.rs`).
  Accept: old config JSON parses.
- [ ] **P2.2** `feat(model-gateway): map scores to tiers` · sonnet · after P2.1
  Files: `services/model-gateway/src/routing.rs`.
  Do: pure core for Auto steps 2 to 5 (cut-off defaults and range are in `constants.rs`, bounds in `askbar-proto`; no literals): cutoffs, sticky (only up), user-set tier untouched, ceiling, budget step-down at the configured point; returns tier and `reasons` (`sticky`, `user_choice`, `budget_stepdown`).
  Accept: table-driven tests covering each rule and their order.
- [ ] **P2.3** `feat(model-gateway): add a System One decision client` · sonnet · after P2.1, G2
  Files: `services/model-gateway/src/decider.rs` (new), `tests/fixtures/decider/<provider>/`.
  Do: one client with a per-provider adapter for URL, auth and response quirks: TypeSafe (`POST /v1/systemone`), Cloudflare Workers AI (`.../ai/run/@cf/cloudflare/clef`; selector `clef-flash` for the 9B) and OpenRouter (`POST /api/alpha/decisions`); start with the provider chosen at G2, each other is one adapter step; capture each provider's request and response shapes (A10) as fixtures; `choice`, `score`, `noul` types; uses only the probabilities and ignores `confidence`, which is not portable; providers tried in order inside `timeout_ms` (default 800 ms and range from `askbar-proto`, set in `constants.rs`; no literal); any failure returns `decider_error`; key held in memory only, redacted in `Debug`, `/status` and logs.
  Accept: fixture tests; a test proves the key is absent from every output; base URL is configurable (a self-hosted OpenJev or Clef uses the client with its own URL); a test shows `confidence` has no effect on the result.
- [ ] **P2.3b** `feat(model-gateway): add a second decider adapter` · sonnet · after P2.3
  Files: `services/model-gateway/src/decider.rs`, `tests/fixtures/decider/<provider>/`.
  Do: the adapter and fixtures for the next provider among TypeSafe, Cloudflare Workers AI and OpenRouter; repeat as one step per extra provider.
  Accept: fixture tests per provider; the same questions give the same typed results through either adapter.
- [ ] **P2.4** `test(e2e): add a mock decider` · haiku · after P2.3
  Files: `tests/e2e/mock_llm.py`.
  Do: serve the System One routes each adapter uses with the same behaviour keywords (slow, 500, hang, low confidence).
  Accept: used by P2.6 tests.
- [ ] **P2.5** `feat(model-gateway): build the chat questions and the state the decider sees` · sonnet · after P2.3
  Files: `services/model-gateway/src/decider.rs`, tests.
  Do: state = latest user turn, a short summary (A2), current tier, context size, attachment types; questions `tier`, `effort`, `unsafe`; score = `tier / 2`. A local-model conversation goes to a self-hosted decider, else `default_tier`, unless `local_to_cloud`.
  Accept: a test asserts keys, memory and other turns outside the summary never appear in the request body.
- [ ] **P2.6** `feat(model-gateway): ask the decider in shadow beside the chosen model` · sonnet · after P2.2, P2.4, P2.5
  Files: `services/model-gateway/src/{lib,llm,decider}.rs`, `tests/streaming.rs`.
  Do: when `chat_auto.mode` is `shadow`, run the decider in a background task per chat; a decider call opens its own audit span first and counts against the user's budget at the admin's price; skip when the auditor does not acknowledge; a hosted decider is refused unless `hosted_approved`. The record gains `score`, `tier_raw`, `decider`, `decider_scores` (probabilities only), `reasons` (`decider_timeout`, `decider_error`). Never delays or changes the chat.
  Accept: with the mock hung, chat latency is unchanged; no prompt text in any attribute; kill switch stops shadow calls.
- [ ] **P2.7** `feat(admin): add decider settings and the zero-retention record` · sonnet · after P2.1
  Files: `services/admin/src/{decider,secrets,db,gateway_link}.rs`, OpenAPI.
  Do: store each decider key as provider keys are stored; providers and order, timeout, thresholds (editable within the `constants.rs` ranges, validated before pushing); a zero-retention record (date, who recorded it, note); the API refuses `hosted_approved` without that record; each change is an admin action; push to the gateway.
  Accept: tests for refusal, push, key never returned.
- [ ] **P2.8** `feat(admin): store labeled examples` · sonnet · after P2.1
  Files: `services/admin/src/{routing,db}.rs`, OpenAPI.
  Do: append a migration for examples (text, tier, source, created); create, list, delete, and JSONL import through the API; size limits are named constants in `constants.rs`.
  Accept: import test with bad rows; deleted with the admin's data only (user-linked shares come in Phase 3).
- [ ] **P2.9** `feat(admin): push labeled examples to the gateway` · haiku · after P2.8, P2.7
  Files: `services/admin/src/gateway_link.rs`.
  Do: include examples in the pushed config; if the body would pass the gateway's request limit, fail with a clear message.
  Accept: push test with 300 examples.
- **P2.10, P2.11, P2.16, P2.18, P2.21** removed: local classifier dropped.
- [ ] **P2.12** `feat(model-gateway): report accuracy on held-back examples` · sonnet · after P2.6, P2.9
  Files: `services/model-gateway/src/{decider,lib}.rs`, `docs/openapi/model-gateway.v1.yaml`.
  Do: admin-listener `POST /check`: hold back 20% and run it through each configured decider; report accuracy, frontier questions sent to fast, cost against always-`standard`, p95 latency, and suggested cutoffs.
  Accept: deterministic split; OpenAPI test.
- [ ] **P2.13** `feat(model-gateway): add a dry-run route` · haiku · after P2.12
  Files: `services/model-gateway/src/lib.rs`, OpenAPI.
  Do: `POST /dry-run` on the admin listener: score, cutoffs and tier for one prompt per configured decider; a decider call is audited as in P2.6.
  Accept: no provider call for the chat itself.
- [ ] **P2.14** `feat(admin): expose check and dry-run` · haiku · after P2.13
  Files: `services/admin/src/routing.rs`, one route line each in `lib.rs`, OpenAPI.
  Do: `POST /api/admin/v1/routing/check` and `/dry-run` pass through to the gateway.
  Accept: tests with a fake gateway.
- [ ] **P2.15** `feat(admin): report shadow results from decision records` · sonnet · after P2.6, P1.6
  Files: `services/admin/src/routing.rs`, OpenAPI.
  Do: `GET /api/admin/v1/routing/shadow-report`: per decider, share of chats where its tier differs from the chosen model's tier, estimated cost against always-`standard`, decider error and timeout rates; from decision records only.
  Accept: fixture records give expected numbers.
- [ ] **P2.17** `docs(adr): amend 0006: private route to the self-hosted decider host` · haiku · after G5. Files: `0006-v1-outbound-access-and-alerts.md`. Gate: any network change is a stop-and-ask.
- [ ] **P2.19** `docs(docs): name the hosted decider's provider in the privacy notice` · haiku · after P2.7
  Files: the notice file (`grep -rln "privacy notice" docs`).
  Do: what the decider sees, that every Auto chat sends its turn to the decider, that a hosted decider is off by default, and the training-share wording. Only live with G4.
  Accept: docs check.
- [ ] **P2.20** `docs(adr): amend 0028: the decider key is pushed like provider keys` · haiku · after P2.7. Files: `0028-service-boundaries-and-config-delivery.md`.
- [ ] **P2.22** `docs(adr): amend 0013: labeled examples are in admin.db` · haiku · after P2.8. Files: `0013-backups.md`.
- [ ] **P2.23** `docs(adr): amend 0054: decider client, check and dry-run` · haiku · after P2.14. Files: `0054-model-gateway.md`.
- [ ] **P2.24** `docs(adr): amend 0040: decider settings and examples` · haiku · after P2.14. Files: `0040-admin.md`.

Phase end: PR. The owner reviews `shadow-report` and the check report before Phase 3 go-live (G7).
