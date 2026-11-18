# Phase 3: Auto (branch `feat/routing-auto`)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

Routed calls, Auto in chat, the client controls. Auto stays hidden until G7.

- [ ] **P3.1** `feat(model-gateway): skip a failing model for five minutes` · sonnet · after Phase 2
  Files: `services/model-gateway/src/routing.rs`.
  Do: shared health map keyed by model, `skip_failing_s` (300 s default and range in `constants.rs`, bounds in `askbar-proto`), thread-safe, cleared on config push.
  Accept: concurrency test; skip expires.
- [ ] **P3.2** `feat(model-gateway): fall back within the tier before the first token` · sonnet · after P3.1
  Files: `services/model-gateway/src/llm.rs`, `tests/streaming.rs`, `tests/e2e/mock_llm.py` keywords.
  Do: for routed calls (A8) try the next-weighted model, up to `max_tries` (3 by default, range in `constants.rs`, bounds in `askbar-proto`); each try is an Attempt span and checks the cost cap; never local to cloud unless `local_to_cloud`; a failure after the first token ends the call; `fallback` goes in `reasons`.
  Accept: tests with 429, 500, reset, hang; a named model's behaviour unchanged.
- [ ] **P3.3** `feat(model-gateway): map effort to each provider's field` · haiku · after P1.2
  Files: `services/model-gateway/src/llm.rs` (`build_body`), tests.
  Do: `output_config.effort` for Claude, `reasoning_effort` for OpenAI-compatible; nearest listed level; none for a model without levels; record when adjusted or ignored.
  Accept: body tests per provider type.
- [ ] **P3.4** `feat(model-gateway): route Auto chats` · sonnet · after P2.2, P3.2, P3.3
  Files: `services/model-gateway/src/{lib,llm,routing}.rs`, `tests/api.rs`.
  Do: `model: "auto"` runs Auto steps 1 to 7; while `mode` is `shadow` an Auto request uses `default_tier`; unsafe answer refuses with A7; effort from the user, the decider's `effort`, else (only when the decider fails) the heuristic (turn length, context size, tool-loop depth); full decision record, `tier_final`.
  Accept: tests for sticky, ceiling, user-set tier, decider timeout to `default_tier`.
- [ ] **P3.5** `feat(model-gateway): step Auto down at the chat budget point` · sonnet · after P3.4, P1.13
  Files: `services/model-gateway/src/{routing,usage}.rs`.
  Do: one tier down at the step-down point (90% by default; value in `constants.rs`, bounds in `askbar-proto`) of the monthly budget in the active unit; never for a user-set tier.
  Accept: `budget_stepdown` recorded; 100% still stops.
- [ ] **P3.6** `feat(model-gateway): run the decider beside the gates` · sonnet · after P3.4, P2.5
  Files: `services/model-gateway/src/{lib,decider}.rs`.
  Do: start the decider call alongside step 1; wait at most `timeout_ms`; one call returns tier, effort and unsafe; on failure effort comes from the heuristic.
  Accept: first token delayed by at most the timeout against a slow mock; failure takes `default_tier` with `decider_error`.
- [ ] **P3.7** `feat(model-gateway): queue shared questions in memory` · sonnet · after P3.4
  Files: `services/model-gateway/src/{lib,share}.rs` (new), OpenAPI.
  Do: bounded in-memory queue (size is a value in `constants.rs`, bounds in `askbar-proto`) fed by `share: true`; admin-listener drain route; an audit record per share (user, time, decision id, no text).
  Accept: a test puts a marker prompt in the queue and greps every log and audit attribute for it; a full queue drops oldest and counts.
- [ ] **P3.8** `feat(server): store a conversation's tier and effort` · sonnet · after P1.1
  Files: `services/server/src/{store,space}.rs`, new migration, OpenAPI.
  Do: append migration for `tier`, `tier_user_set`, `effort`; `model` may be `auto`; validate tier against the ceiling.
  Accept: old conversations load; migration test.
- [ ] **P3.9** `feat(server): send roles and tiers to the gateway` · sonnet · after P3.8, P3.4
  Files: `services/server/src/{gateway,generation}.rs`.
  Do: Auto chats send `role: chat.auto`, `tier`, `tier_user_set`, `effort`, `share`; store `tier_final` from the first event; old gateway still works.
  Accept: tests against both gateway versions.
- [ ] **P3.10** `feat(server): set a conversation's tier and effort` · haiku · after P3.8
  Files: `services/server/src/*` (route), `docs/openapi/client.v1.yaml`, `docs/wire-contract.md`.
  Do: follow "Adding an endpoint, end to end"; additive.
  Accept: OpenAPI test.
- [ ] **P3.11** `feat(server): ask again on a stronger model` · sonnet · after P3.9
  Files: `services/server/src/generation.rs`, OpenAPI.
  Do: re-send one tier up; sets `share` per `training_share` (on `ask` only with the user's consent in the request); marks the message shared.
  Accept: tests for both modes and for the top tier.
- [ ] **P3.12** `feat(admin): collect shared questions` · sonnet · after P3.7
  Files: `services/admin/src/{routing,db}.rs`, user delete path, OpenAPI.
  Do: poll the drain route; append migration `shared_questions`; API to list, label and delete; deleted with the user, labeled or not.
  Accept: user-delete test removes them; backup docs updated in P3.24.
- [ ] **P3.13** `feat(admin): set Auto routing options` · sonnet · after P2.7, P3.4
  Files: `services/admin/src/{routing,gateway_link}.rs`, OpenAPI.
  Do: cutoffs, `default_tier`, ceiling (A5), mode, training share, fallback settings, role pins for chat; validation against the `constants.rs` ranges; push.
  Accept: a pin to an unapproved model is refused before pushing.
- [ ] **P3.14** `feat(admin): lower accounts near their subscription limit` · sonnet · after P1.17, P3.4
  Files: `services/admin/src/{subscriptions,gateway_link}.rs`.
  Do: at 90% an account's models get weight 1, at 100% unavailable until reset (points from `constants.rs`); on by default, switchable off; each change an admin action.
  Accept: tests for 90, 100, reset, off.
- [ ] **P3.15** `feat(model-gateway): list Auto in the catalog when live` · haiku · after P3.4
  Files: `services/model-gateway/src/lib.rs`, `tests/models.rs`, OpenAPI.
  Do: `GET /catalog` adds an `auto` entry only when `mode` is `live`.
  Accept: old shape unchanged otherwise.
- [ ] **P3.16** `feat(server): default new chats to Auto when listed` · haiku · after P3.15
  Files: `services/server/src/*`.
  Do: if the catalog has `auto`, new chats default to it; chosen models still skip routing.
  Accept: test with and without the entry. Takes effect only at G7.
- [ ] **P3.17** `feat(client): show Auto and a reply's tier` · sonnet · after P3.9
  Files: `clients/desktop/ui/*`, `clients/desktop/src/*`.
  Do: Auto in the picker, tier label on replies; text via `textContent`; follow `docs/style/ui-ux.md`.
  Accept: client checks and `npm test`.
- [ ] **P3.18** `feat(client): add tier and effort controls` · sonnet · after P3.10, P3.17
  Do: per-conversation tier (within ceiling) and effort (Auto, low, medium, high); sticky.
  Accept: UI test.
- [ ] **P3.19** `feat(client): ask again and the shared marker` · sonnet · after P3.11, P3.17
  Do: "Ask again on a stronger model"; the ask-first dialog for `ask` mode; the marker on shared messages.
  Accept: UI test.
- [ ] **P3.20** `test(model-gateway): Auto end to end` · haiku · after P3.9, P3.15
  Files: `scripts/smoke.sh`, `tests/e2e/mock_llm.py`.
  Do: smoke Auto with sticky tier, step-down, fallback, shared question.
  Accept: smoke passes.
- [ ] **P3.21** `test(model-gateway): measure decider latency against the 200 ms target` · haiku · after P3.6
  Do: a script that records the added time for the mock decider and, with keys, each configured provider; p95 per provider in the PR, no CI gate.
- [ ] **P3.22** `docs(docs): describe Auto in the guide and the wire contract` · haiku · after P3.19. Files: `docs/guide.md`, `docs/wire-contract.md`.
- [ ] **P3.23** `docs(adr): amend 0054: routing, fallback, effort, share queue` · haiku · after P3.7. Files: `0054-model-gateway.md`.
- [ ] **P3.24** `docs(adr): amend 0013: shared questions in admin.db` · haiku · after P3.12. Files: `0013-backups.md`.
- [ ] **P3.25** `docs(adr): amend 0039: roles, tiers and ask again` · haiku · after P3.11. Files: `0039-server.md`.
- [ ] **P3.26** `docs(adr): amend 0040: routing settings, shared questions, account demotion` · haiku · after P3.14. Files: `0040-admin.md`.
- [ ] **P3.27** `docs(adr): amend 0044: Auto, tier and effort controls` · haiku · after P3.19. Files: `0044-desktop-client.md`.
- [ ] **P3.28** `docs(adr): amend 0023: request_blocked problem type` · haiku · after P3.4. Files: `0023-api-error-format.md`.

Phase end: PR. G7: the owner flips `mode` to live in the admin API; nothing in this plan does it.
