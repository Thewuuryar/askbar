# Phase 1: records only (branch `feat/routing-records`)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

Models, tiers, prices, decision records, dollar budgets and subscription reads. No routing change: every call goes where it goes today. Needs G0 and G1. Every behaviour value a step adds goes in `constants.rs`, bounds in `askbar-proto` when a service checks it.

- [ ] **P1.1** `feat(proto): add routing config and chat request fields` · haiku · after G1
  Files: `crates/askbar-proto/src/gateway.rs`, tests.
  Do: add `Tier`, `Role`, `Effort`, `ModelRouting {input, output, cached_in, effort, tier, weight}`, `RoutingConfig`, `DecisionRecord`; `ChatRequest` gains optional `role`, `tier`, `tier_user_set`, `task_id`, `effort`, `share`; the first SSE `span` event gains optional `decision_id`, `tier_final`. All `#[serde(default, skip_serializing_if)]`.
  Accept: tests parse the old `GatewayConfig`, `ChatRequest` and event JSON unchanged, and the new ones round-trip.
- [ ] **P1.1a** `feat(proto): add shared bounds and defaults for routing values` · haiku · after P1.1, G0
  Files: `crates/askbar-proto/src/gateway.rs`, tests.
  Do: keep today's grouping by topic (general defaults in `lib.rs`, audit in `audit.rs`): put each routing constant in `gateway.rs` next to the `GatewayConfig` field it bounds; if the file grows unwieldy, split them into a `gateway/limits.rs` submodule re-exported from `gateway`. Don't move existing constants. `MIN_`/`MAX_`/`DEFAULT_` constants for every value a service checks: weight range 1 to 100, `decider.timeout_ms`, cut-off range, `max_tries`, `skip_failing_s`, chat budget step-down point, task tool-call default 8 and range 1 to 32, research waits (8 s, 90 s) and summary size (800 tokens), approval expiry (60 s), library thresholds, decider thresholds. Doc comment per value.
  Accept: constants compile and are unit-tested for `MIN <= DEFAULT <= MAX`; nothing else changes.
- [ ] **P1.1e** `refactor(proto): share the generation timeout maximum` · haiku · after G0
  Files: `crates/askbar-proto/src/lib.rs`, `services/server/src/snapshot.rs`, `services/admin/src/constants.rs`.
  Do: move `MAX_GENERATION_TIMEOUT_SECS` (3600, today defined separately in the server's `snapshot.rs` and the admin's `constants.rs`) into `lib.rs` beside `DEFAULT_GENERATION_TIMEOUT_SECS`; both services use or re-export it. Do the same for any other bound the two copy, found with a grep for matching `MIN_`/`MAX_` names.
  Accept: one definition per bound; behaviour unchanged; existing tests pass untouched.
- [ ] **P1.1b** `feat(admin): add routing values to constants.rs` · haiku · after P1.1a
  Files: `services/admin/src/constants.rs`, its tests.
  Do: every routing behaviour value, each with MIN_/MAX_/DEFAULT_ and a doc comment: the P1.1a values (re-exported from `askbar_proto`, as `DEFAULT_GENERATION_TIMEOUT_SECS` is), plus admin-only ones: price bands for tier suggestion, budget and subscription alert points (75, 90; the 100% stop), subscription poll interval (5 min). Add them to the struct served as `/admin/constants.json`. Nothing secret.
  Accept: a test checks the served JSON has no secret-looking keys (`key`, `token`, `secret`, `password`, `prompt`); existing constants tests pass untouched.
- [ ] **P1.1c** `feat(admin): push constants in each service's config` · sonnet · after P1.1b
  Files: `services/admin/src/{gateway_link,server_link}.rs`, `crates/askbar-proto/src/gateway.rs`, `services/model-gateway/src/{lib,models}.rs`, `services/server/src/snapshot.rs`, tests.
  Do: each link includes only the values that service uses, as additive defaulted fields inside its existing config push (`PUT /config`, the snapshot); admin-edited settings override the default within the range, validated before pushing; the config version hash covers them. The receiving side rejects a value outside the `askbar-proto` bounds and keeps its old config; before its first push it uses the `askbar-proto` defaults. Hard floors set by ADRs (audit retention at least 7 days) stay compiled into the enforcing service too.
  Accept: tests: a changed constant changes the version and re-pushes; an out-of-bounds value is rejected and the old config stays; a service before its first push uses the `askbar-proto` defaults; an old payload without the fields parses.
- [ ] **P1.1d** `docs(adr): amend 0028: constants in pushed config` · haiku · after P1.1c. Files: `0028-service-boundaries-and-config-delivery.md`. Constants travel in each service's pushed config; bounds live in `askbar-proto`. Decision amendment: owner approves in PR.
- [ ] **P1.2** `feat(model-gateway): accept and validate routing config` · sonnet · after P1.1, P1.1a
  Files: `services/model-gateway/src/{lib,models}.rs`, `tests/{models,api}.rs`, `docs/openapi/model-gateway.v1.yaml`.
  Do: store `routing` from `PUT /config`; reject the push (existing rejection path) for an unapproved model, unknown tier, weight outside the `askbar-proto` range (1 to 100; defaults and range live in `constants.rs`, no literals), negative price; show routing summary and version in `/status` (no keys).
  Accept: bad pushes leave the old config live; version hash changes with routing; OpenAPI test passes.
- [ ] **P1.3** `feat(model-gateway): add weights and in-tier choice` · haiku · after P1.2
  Files: `services/model-gateway/src/routing.rs` (new), `lib.rs` (mod line).
  Do: pure functions: blended price (60% input, 40% output), weight `100*(pmin+0.01)/(p+0.01)` rounded and clamped to the weight range (1 to 100, from `askbar-proto`; `constants.rs` holds the default and range), override wins, `choose(tier, available)` = highest weight, ties to lower price.
  Accept: unit tests for $0 vs $1 (about 1), ties, override, empty tier.
- [ ] **P1.4** `feat(model-gateway): record a routing decision on each chat span` · sonnet · after P1.2
  Files: `services/model-gateway/src/{lib,llm}.rs`, `tests/streaming.rs`, `docs/style/logging-and-audit.md`, `docs/openapi/model-gateway.v1.yaml`.
  Do: build the decision record at span start: `decision_id` (`rd_` + ULID, `ulid` is already a dependency), `config_version`, `role` (A1), `requested`, `model`, `tier_final` from the model's tier if set, `estimated_cost` from prices, `reasons` empty. Attributes only, never prompt text. Return `decision_id` and `tier_final` in the first SSE event.
  Accept: the span attributes carry the record; the audit-before-call order is unchanged; routing is unchanged; a test greps the log output for a marker prompt and finds nothing.
- [ ] **P1.5** `feat(auditor): look up spans by decision id` · sonnet · after P1.4
  Files: `services/auditor/src/*` (find the read route), `docs/openapi/audit.v1.yaml`.
  Do: first check whether the audit read API can already filter by an attribute. If so, tick "not needed" and add only a test. If not, add an additive `decision_id` query filter.
  Accept: a stored span is found by `decision_id`; the chain and sealing are untouched.
- [ ] **P1.6** `feat(admin): add a routing decision lookup` · sonnet · after P1.5, G0
  Files: `services/admin/src/routing.rs` (new), one route line in `lib.rs`, `docs/openapi/admin.v1.yaml`, tests.
  Do: `GET /api/admin/v1/routing/decisions/{id}` through the audit read token (`audit_read.rs`); returns the record fields only. Admin records the read as an admin action.
  Accept: 404 for unknown ids; OpenAPI test passes; no `pages.rs`, `web.rs` or `ui/**` edits.
- [ ] **P1.7** `feat(server): relay the decision id to clients` · sonnet · after P1.4
  Files: `services/server/src/{gateway,generation}.rs`, `docs/openapi/client.v1.yaml`, `docs/wire-contract.md`.
  Do: parse the new first-event fields (ignore if absent) and add `decision_id` and `tier_final` to the client's stream, additively.
  Accept: old gateway (no fields) and new gateway both stream; tests for both.
- [ ] **P1.8** `feat(admin): move prices to each model` · sonnet · after P1.1, G0
  Files: `services/admin/src/{db,catalog,cost,admin_api}.rs`, `docs/openapi/admin.v1.yaml`.
  Do: append a migration adding per-model `price_in`, `price_out`, `price_cached_in`, filled from the provider's prices; `cost.rs` uses the model's price, else the provider's; the models API shows and sets them. Provider prices stay until the cost views no longer read them.
  Accept: cost totals unchanged for existing data; migration test on a copy of an old database.
- [ ] **P1.9** `feat(admin): add tiers, effort levels and weight overrides to models` · sonnet · after P1.8, P1.1b
  Files: `services/admin/src/{db,catalog,routing}.rs`, OpenAPI.
  Do: append a migration for `tier`, `effort_levels`, `weight_override` (the override is checked against the weight range in `constants.rs`); `suggest_tier()` from the price bands in `constants.rs` (A4; added in P1.1b, no literals here) or the parameter count Ollama reports (read it where the catalog already queries Ollama); API shows default weight and override side by side.
  Accept: suggestion tests per band; an unconfirmed tier is "suggested", not active.
- [ ] **P1.10** `feat(admin): push routing config to the gateway` · sonnet · after P1.2, P1.9, P1.1c
  Files: `services/admin/src/gateway_link.rs`, tests.
  Do: add `routing.models` to the pushed `GatewayConfig` beside the constants from P1.1c; keep the version-mismatch re-push every 12 s (the interval is a named value in `constants.rs`); a rejected push shows its error and does not loop faster than the interval.
  Accept: a fake gateway receives the routing section; an old gateway (ignores it) still accepts; version stays stable across identical pushes.
- [ ] **P1.11** `test(model-gateway): records-only end to end` · haiku · after P1.4, P1.7, P1.10
  Files: `tests/e2e/mock_llm.py` (if needed), `scripts/smoke.sh`.
  Do: smoke step: set prices and tiers, chat, check `decision_id` in the stream and the record via the admin lookup; the chosen model is the one asked for.
  Accept: `scripts/smoke.sh` passes.
- [ ] **P1.12** `feat(auditor): return token counts per model` · sonnet · after P1.4
  Files: `services/auditor/src/*`, `docs/openapi/audit.v1.yaml`, `usage.rs` fixtures.
  Do: check whether `/token-counts` already splits input, output and cached-input per model. If yes, tick "not needed". If not, add them as extra fields.
  Accept: old consumers still parse.
- [ ] **P1.13** `feat(model-gateway): compute spend in dollars` · sonnet · after P1.12, P1.2
  Files: `services/model-gateway/src/{usage,lib}.rs`, `tests/api.rs`, OpenAPI.
  Do: dollar cost from per-model prices; config `budget.unit` (`tokens` default, `dollars`) and optional `daily_usd`, `monthly_usd` per user (any new limit or interval, such as a maximum dollar amount, goes in `constants.rs` with bounds in `askbar-proto`); `gates()` uses the active unit; both are tracked every 60 s. Under `dollars`, a model with no price refuses the call (fail closed).
  Accept: every existing token-budget test passes untouched; new tests for dollars, missing price, and the unit switch mid-day.
- [ ] **P1.14** `feat(admin): set budgets in dollars and switch the unit` · sonnet · after P1.13, P1.10
  Files: `services/admin/src/{admin_api,budget_watch,db}.rs`, OpenAPI.
  Do: user budget fields in dollars (limits in `constants.rs`); a unit switch the admin refuses while any approved model lacks a price; `budget_watch` computes in the active unit; an admin action records the switch. Token budgets stay until the owner switches. The owner enters dollar amounts; there is no automatic conversion.
  Accept: with `tokens` nothing changes; with `dollars` alerts and gates follow dollars.
- [ ] **P1.15** `feat(admin): alert at 75 and 90 percent of a budget` · haiku · after P1.14, P1.1b
  Files: `services/admin/src/budget_watch.rs`, its tests.
  Do: thresholds 80/100 become 75/90, read from the alert-point constants in `constants.rs` (P1.1b), not literals; keep the 100% "reached" alert (A9). Update the tests and say in the commit body that the contract changed.
  Accept: each alert fires once per threshold per period.
- [ ] **P1.16** `feat(admin): read subscription usage through cpa` · sonnet · after G0
  Files: `services/admin/src/cpa.rs`, `tests/fixtures/`.
  Do: add `api_call()` for `POST /v0/management/api-call` (any timeout goes in `constants.rs`, not a literal); a code-level allowlist of exactly `api.anthropic.com/api/oauth/usage` and `chatgpt.com/backend-api/wham/usage`; parsers for 5-hour, 7-day and per-family percent and reset times; any failure returns `Unknown`.
  Accept: fixtures from the design's sources parse; a third address is refused in code; the admin never reads the account token.
- [ ] **P1.17** `feat(admin): poll subscription limits and estimate tokens left` · sonnet · after P1.16, P1.8, P1.1b
  Files: `services/admin/src/subscriptions.rs` (new), `db.rs` migration, one spawn line in `lib.rs`.
  Do: every poll interval (5 minutes, from `constants.rs`) list `auth-files`, read each account; store snapshots; tokens-per-1% estimator from Askbar's tokens per account and model between reads (from the auditor counts); low confidence when percent moves with no Askbar traffic.
  Accept: estimator unit tests (including zero change, a reset, and outside traffic); a failed read never blocks model calls.
- [ ] **P1.18** `feat(admin): alert at 75 and 90 percent of a subscription window` · sonnet · after P1.17
  Files: `services/admin/src/{subscriptions,alerter_link}.rs`.
  Do: one alert per window per threshold (75 and 90, the alert-point constants in `constants.rs`) through the `alerter`, deduplicated across restarts by stored state; one "unknown" alert per outage.
  Accept: tests for repeat, reset window, restart.
- [ ] **P1.19** `feat(admin): add a subscription limits route` · haiku · after P1.17
  Files: `services/admin/src/routing.rs` or `subscriptions.rs`, one line in `lib.rs`, OpenAPI.
  Do: `GET /api/admin/v1/subscriptions`: windows, percent, reset, tokens left per model, confidence.
  Accept: OpenAPI test.
- [ ] **P1.20** `docs(adr): amend 0054: routes, prices and decision records` · haiku · after P1.4. Files: `0054-model-gateway.md`. Description.
- [ ] **P1.21** `docs(adr): amend 0051: decision fields and lookup` · haiku · after P1.5. Files: `0051-auditor.md`. Description.
- [ ] **P1.22** `docs(adr): amend 0042: usage reads through api-call` · haiku · after P1.16. Files: `0042-cpa.md`. Description.
- [ ] **P1.23** `docs(adr): amend 0039: relay the decision id` · haiku · after P1.7. Files: `0039-server.md`. Description.
- [ ] **P1.24** `docs(adr): amend 0040: model prices, tiers, dollar budgets, subscription reads, constants.rs` · haiku · after P1.19, P1.1c. Files: `0040-admin.md`. Description; also records that the admin owns `constants.rs` (every routing behaviour value) and pushes each service the values it uses.

Phase end: full checks, PR. Owner switches the budget unit later, when prices are entered.
