# Plan: tiered model routing

Status: proposed, after 1.0. Nothing here is decided until [ADR](adr/README.md) 0056 and the amendments in Phase 0 are accepted. This file replaces the older local-only classification plan of the same name and is its progress tracker.

Design: [design-tiered-routing.md](design-tiered-routing.md) (committed in P0.1 from the design file the owner gives the first session). The design wins on behaviour; this plan wins on order. The new ADR is 0056 (0055 is the alerter).

Deferred: every admin web UI task (routing page, decider settings page, dry-run panel, decider comparison, shared-question review, spend views, MCP server and tool permission pages, agent task views) waits for Phase 8, which may start any time after G0 (`claude/trusting-thompson-3vv2sw`, the admin UI overhaul, merged into `main`). Admin backend work from Phase 1 on also waits for G0: new values go straight into `services/admin/src/constants.rs`, which exists only on that branch. Phase 0 (docs) can run before G0. Admin backend API work goes in new modules, without touching `pages.rs`, `web.rs`, `ui/**` or the route table beyond one line per route in `lib.rs`.

Also deferrable: the executor (Phase 9) and the script library (Phase 10, the script review page P8.7 with it) come after Phase 8 and start only when the owner asks. Phases 5A, 5B and 6 ship agents with MCP tools and no executor; the executor and the script library are not needed for them.

## Starting a session (owner)

Start Claude Code on Sonnet and send "You are the orchestrator for `docs/plan-tiered-routing.md`. Follow its How to run section from the first unchecked step." Start a new session at each phase, or when the context is long; the ticked boxes carry the state.

## How to run (orchestrator)

You are a Sonnet orchestrator. You do not write step code; subagents do. You read, brief, check, review, commit and tick.

**Loop**

1. Read this file. Pick the first unchecked step whose `after` steps are all ticked and whose gate (if any) is cleared. If the gate is open, stop and ask the owner; do not skip ahead past a gate.
2. Branch: one branch per phase, `<type>/<kebab>` as named in the phase heading. Create it from `main` (or from the previous phase's branch if that PR is still open; say so in the PR). Never push to `main`, never merge, never edit `.github/workflows/`.
3. Spawn the subagent named in the step (`model: "haiku"` or `"sonnet"`, foreground) with the brief below. One subagent per step. Do not commit or push while it runs.
4. When it hands back: run the checks for the step type, read `git show --stat HEAD` and the diff against the step's "Accept" line and the review list. Send fixes to the same agent (it amends; nothing is pushed yet).
5. Commit with the exact subject (the agent commits; you verify). Body: why, deviations, any test changed and why. End with the attribution lines your session was given. From P0.2 on, tick the box in `docs/plan-tiered-routing.md` in the same commit.
6. Push the phase branch after each step (`git push -u origin <branch>`; never force). At phase end run the full checks, open one PR titled with the phase's commit-style subject, fill the PR template, say what ran and what did not, say "rebase-merge, each commit stands alone", and stop for the owner.

**Subagent brief** (keep it small): the step block verbatim; "read AGENTS.md (hard rules, stop-and-ask) and the CONTRIBUTING section named in the step"; the file list from the step and nothing else to read unless needed; "one commit with this exact subject, tests and docs in the same commit, do not push, do not add dependencies, do not touch `.github/`, stop and report if the step needs a decision this file does not make"; the check commands; "final report: commit hash, files, check results, deviations".

**Checks per step**

| Touches | Run |
| --- | --- |
| Rust crate or service | `cargo fmt --all --check`, `cargo clippy -p <crate> --all-targets --locked -- -D warnings`, `cargo test -p <crate>` (OpenAPI tests run with it) |
| `clients/desktop` | the same three inside `clients/desktop`, then `npm test` |
| Docs or ADR only | `scripts/check-docs.sh` |
| Dependency added | `cargo deny check`; `node scripts/check-npm-licenses.js package-lock.json clients/desktop/package-lock.json` |
| `compose.yaml`, Dockerfile, network, volume | `scripts/check-isolation.sh` (needs Docker) |
| Phase end | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace`, the same three in `clients/desktop` if touched, `npm test`, `scripts/smoke.sh`, plus the rows above |

**Review list**: no prompt text, reply text, provider key or signing key in logs, errors, `/status` or audit attributes (only sealed payloads carry text); every model or decider call has its audit span acknowledged before the call; new fields are additive with `#[serde(default)]` and old payloads still parse; no literal behaviour values (limits, intervals, thresholds, defaults) in service code; no earlier migration edited; no test loosened without a stated contract change; one concern per commit; OpenAPI and docs updated with the code.

**Haiku or Sonnet.** Haiku: docs, ADR amendment text, OpenAPI and proto boilerplate, test fixtures and mock extensions, grep or explore tasks, mechanical renames, pure functions with a given formula. Sonnet: gateway, server and admin logic, anything concurrent, security, crypto or audit-path code, anything with a migration. Owner: everything under "Owner gates". If a step marked haiku turns out to need judgment, move it to Sonnet; do not ask Haiku twice.

**Retries.** A check fails: the same agent gets one fix attempt. Still failing: Haiku steps go to a fresh Sonnet agent with the diff and the failing output; Sonnet steps stop. Then stop and ask the owner. An unexplained failing test stops the step at once.

**Stop and ask.** A new dependency outside the licence allowlist (and any new dependency named below); a new container, port, network or volume; any hard-rule change; a missing or unaccepted ADR; a decision this file marks "assumed" that the work contradicts; an unexplained failing test. Never accept an ADR, merge, or batch ADR amendments: one amendment, one commit, `docs(adr): amend NNNN: <what>`, edited in place with an Amendments line. Decision amendments need the owner's approval in the PR.

**Compatibility rules for every step.** Proto and API changes are additive (`#[serde(default)]`, optional fields, no renames); an old server with a new gateway, and the reverse, keep working; a test parses the old payload shape. Token budgets keep working until dollar budgets are switched on. Never rename a volume; append migrations. No literal behaviour values in service code: every new limit, interval, threshold or default lives in `services/admin/src/constants.rs`, with its bounds and default in `askbar-proto` when a service also checks it (the admin re-exports them). Pushed constants are additive and defaulted; a service before its first push uses the `askbar-proto` defaults.

## Owner gates

| Gate | Needed before | What |
| --- | --- | --- |
| G0 | P1.1a and every admin backend step from Phase 1 on (Phase 0 can run before it) | `claude/trusting-thompson-3vv2sw` (admin UI overhaul, holds `services/admin/src/constants.rs`) merged into `main` |
| G1 | Phase 1 | Accept ADR 0056 (Proposed to Accepted); approve the decision-amendment PR (Phase 0) |
| G2 | P2.3 | TypeSafe or Cloudflare (or OpenRouter) access and keys for tests; choose the first decider provider |
| G3 | none | removed: local classifier dropped |
| G4 | hosted decider on | Recorded zero-retention agreement with the hosted decider's provider; privacy notice wording |
| G5 | P2.17 | Self-hosted decider host (OpenJev or Clef), licence (OpenJev weights are CC BY-NC 4.0, so commercial use needs a commercial licence; Clef and Clef-flash: check licence), network route |
| G6 | none | removed: local classifier dropped |
| G7 | Phase 3 go-live | Shadow results meet the bar (under 5% frontier-to-fast misses, cheaper than always-`standard`); owner flips `chat_auto.mode` to live and approves Auto as default |
| G8 | Phase 4 | Egress proxy software and licence; new container; compose and network changes |
| G9 | Phase 5A (P5.3 on) | Ed25519 signing crate; MCP gateway software: agentgateway (Apache-2.0 image behind a Rust front) or, as fallback, IBM ContextForge, as chosen at G13; new containers `mcp-gateway`, `searxng`; accept ADR 0057; where approval verification code lives (ADR 0045 allows, or a new crate) |
| G10 | Phase 9 (deferrable) | Executor host and hardware; new image |
| G11 | Phase 10 (deferrable) | Owner promotes each script; go-live bar met |
| G12 | Phase 8 | same as G0; Phase 8 may start any time after it |
| G13 | P5A.2 | Owner reads the spike result (P5A.1) and picks agentgateway or ContextForge (the unverified items decide) |
| G14 | P5A.14 | Search MCP server and its destinations: searxng and the upstream search engines it reaches through the egress proxy |
| G15 | P5A.15 | Fetch MCP server and its egress rule (any public URL that passes the ADR 0019 fetch rules) |
| G16 | each later MCP server (P5A.16 on) | Owner approves every further MCP server, its credentials and its egress destination, one gate per destination |

## Assumptions to confirm (the design is silent)

- **A1** A chat with a named model records role `chat.manual`.
- **A2** The decider's "short summary of recent turns" is the last 3 turns cut to a fixed length by code, no model call.
- **A3** The decider's tier answer is a score over three levels; score = answer / 2.
- **A4** Price bands for tier suggestion are named constants in `constants.rs`, added in P1.1b and used by P1.9 (`suggest_tier()`); the owner confirms them in the PR.
- **A5** The user ceiling is global in `chat_auto.user_ceiling`, with an optional per-user override pushed by the admin.
- **A6** The server holds a conversation's current tier and sends `tier` plus `tier_user_set`; the gateway is stateless about conversations.
- **A7** An `unsafe` answer at or above `unsafe_block` refuses the chat with a new `ErrorCode` (`request_blocked`, 403) and a plain message.
- **A8** In-tier fallback applies to routed calls (Auto, role, tier); a named model keeps today's same-provider retries.
- **A9** Budget alerts become 75% and 90%; the 100% "budget reached" alert stays.
- **A10** Each provider's System One request and response shapes come from its reference, captured as fixtures in P2.3 and P2.3b.
- **A11** The design and this plan live in `docs/` as `design-tiered-routing.md` and `plan-tiered-routing.md`.
- **A12** The MCP gateway's component name is `mcp-gateway` unless ADR 0057 picks a less confusable one (ADR 0021 rule 4); its API prefix is chosen in ADR 0057 and every step uses the ADR's names.
- **A13** An invalid plan is sent back to the planner once with the validation error codes (never tool output); a second invalid plan stops the task with a plain message. A `schema` check applies to the step's final JSON output.

## Phase 0: records and docs (branch `docs/tiered-routing-adr`)

Docs only. Ends with G1.

- [x] **P0.1** `docs(docs): add the tiered routing design` · haiku · after none
  Files: `docs/design-tiered-routing.md`.
  Do: copy the design file the owner attached (stop and ask if it is missing); make ADR and repo links relative. (The design already uses ADR 0056 and ADR 0001 rules 1 and 2.)
  Accept: the new ADR is cited as 0056 throughout; `scripts/check-docs.sh` passes.
- [x] **P0.2** `docs(docs): replace the tiered routing plan` · haiku · after P0.1
  Files: `docs/plan-tiered-routing.md`, any file linking to it (`grep -rn plan-tiered-routing`).
  Do: overwrite with the attached plan; fix links.
  Accept: docs check passes. From here on tick boxes in the repo copy.
- [ ] **P0.3** `docs(adr): propose 0056 tiered model routing` · sonnet · after P0.1
  Files: `docs/adr/0056-tiered-model-routing.md` (from `0000-template.md`), `docs/adr/README.md` index row.
  Do: Status Proposed. Decision, reasons and consequences from the design (tiers, roles, Jev as a decision provider, signed approvals, script library, rollout, budgets in dollars). Link the amendments below. Do not write "Accepted".
  Accept: ADR format check passes; README row added. Gate: owner accepts (G1).
- [ ] **P0.4** `docs(adr): amend 0014: tiers, deciders, dollar budgets, fallback` · haiku · after P0.3
  Files: `docs/adr/0014-model-gateway-and-providers.md`.
  Do: use the design's ADR table row (catalog tiers and weights, default tier, role pins, deciders, prices and effort, dollars, `auto`, in-tier fallback, an approved decider may see prompts). Add an Amendments line.
  Accept: docs check. Decision amendment: owner approves in PR.
- [ ] **P0.5** `docs(adr): amend 0002: routing after 1.0 and latency budgets` · haiku · after P0.3. Files: `0002-v1-scope-limits-and-ship-criteria.md`. Design row. Decision.
- [ ] **P0.6** `docs(adr): amend 0010: decision records and task spans` · haiku · after P0.3. Files: `0010-auditor.md`. Design row. Decision.
- [ ] **P0.7** `docs(adr): amend 0011: shared questions and hosted decider` · haiku · after P0.3. Files: `0011-sealed-content-access.md`. Two exceptions, each with notice and a record per use. Decision.
- [ ] **P0.8** `docs(adr): amend 0012: notices for sharing and the decider` · haiku · after P0.3. Files: `0012-consent-notices-and-deletion.md`. Decision.
- [ ] **P0.9** `docs(adr): amend 0016: routing and library risks` · haiku · after P0.3. Files: `0016-risk-register.md`. Defend and Accept lists from the design row. Register.
- [ ] **P0.10** `docs(adr): amend 0019: order, judge role, writer and pins` · haiku · after P0.3. Files: `0019-post-v1-order-and-tools.md`. Order: routing, egress proxy, MCP gateway, research and judge, agents with MCP tools, admin UI, executor (deferrable), library (deferrable); hosted Jev on the allowlist if approved. Decision.
- [ ] **P0.11** `docs(adr): amend 0020: approvals, library scripts and the executor deferred` · haiku · after P0.3. Files: `0020-remote-executor-and-ca-triggers.md`. The executor comes after agents with MCP tools and is deferrable. Decision.
- [ ] **P0.12** `docs(adr): amend 0009: task state in the space file` · haiku · after P0.3. Files: `0009-spaces-and-storage.md`. Decision.
- [ ] **P0.13** `docs(adr): amend 0022: the judge is a role, not a caller` · haiku · after P0.3
  Files: `0022-api-path-conventions.md`.
  Do: first `grep -n judge` in 0022; if it lists the judge as a `model-gateway` caller, correct it (the server calls the judge role). If it does not, tick as "not needed" with no commit.
  Accept: matches the architecture edit in P0.14. Decision if it changes a rule.
- [ ] **P0.14** `docs(docs): add tiered routing terms to the architecture` · haiku · after P0.3
  Files: `docs/architecture.md`.
  Do: model-gateway row (line 17); reserved names (line 21): drop judge and add the one-line note, remove it as a gateway caller; glossary (line 23): the design's term table, space changed.
  Accept: docs check; glossary names match the design.
- [ ] **P0.15** `docs(docs): describe Auto, tiers and /decide in the architecture` · haiku · after P0.14
  Files: `docs/architecture.md`.
  Do: "Model choice" (~line 234) gains Auto, tier, effort; budgets in dollars; Links table gains `/decide`. Mark each as post-1.0.
  Accept: docs check.
- [ ] **P0.16** `docs(docs): reorder post-v1 plan for tiered routing` · haiku · after P0.3
  Files: `docs/plan-post-v1.md` (status line 3, V2 sketch lines 5 to 7, mermaid lines 9 to 59).
  Do: new order; the design's V2 sentence; replace the diagram with the design's.
  Accept: mermaid parses (docs check).
- [ ] **P0.17** no commit: update the claude.ai project doc `architecture-goals-and-features.md` · owner · after G1
  Do: the orchestrator has no access to the claude.ai project; the owner asks a claude.ai chat in the "Askbar AI" project to apply the design's four bullets (Clients, Server, Admin, Later), then ticks this box.
  Accept: only after the ADR is accepted.

Phase end: open the PR, mark every decision amendment for approval, stop at G1.

## Phase 1: records only (branch `feat/routing-records`)

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

## Phase 2: shadow (branch `feat/routing-shadow`)

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

## Phase 3: Auto (branch `feat/routing-auto`)

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

## Phase 4: egress proxy (branch `feat/egress-proxy`, coarse)

When the phase starts, a Sonnet planning subagent expands each line into detailed steps (files, accept, agent) and the orchestrator commits that expansion to this file first (`docs(docs): expand egress proxy steps`). Needs G8.

- [ ] **P4.1** `docs(adr): confirm ADR coverage for the egress proxy` · haiku · check ADR 0019 and 0038 name it; if not, the owner decides on a new ADR.
- [ ] **P4.2** `feat(compose): add the egress proxy service` · sonnet · new container, networks; `check-isolation.sh`.
- [ ] **P4.3** `feat(compose): send gateway, cpa, alerter and searxng traffic through the proxy` · sonnet
- [ ] **P4.4** `feat(admin): push per-source allowlists to the proxy` · sonnet · hosted decider addresses if approved; the self-hosted decider host as an approved private address.
- [ ] **P4.5** `test(compose): prove default deny and private-address blocking` · sonnet · extend isolation checks.
- [ ] **P4.6** `docs(adr): amend 0006: egress through the proxy` · haiku
- [ ] **P4.7** `docs(adr): amend 0032: proxy container limits` · haiku
- [ ] **P4.8** `docs(docs): update the architecture network table and diagram` · haiku

## Phase 5A: MCP gateway (branch `feat/mcp-gateway`)

The MCP gateway is agentgateway (Linux Foundation AAIF project, Rust, Apache-2.0) behind a thin Rust front service, following the `cpa` pattern (a third-party image wrapped by a Rust supervisor, ADR 0037; see `services/cpa-supervisor`). agentgateway does MCP federation, transports, tool servers and their credentials, and per-user per-tool CEL rules; its `mcpAuthorization` CEL cannot read tool arguments or call out. The front does what it can't: verify the signed approval (Ed25519 over the exact call hash, one-time ID, 60 s expiry), write the audit span before the call through the Rust `AuditClient` (`crates/askbar-service`; fail closed), check per-user tool permissions, enforce ADR 0019's fetch rules, serve `GET /status` with the config version and take config pushes (ADR 0028). agentgateway listens only inside the container or private network, so nothing reaches it except through the front. Fallback: IBM ContextForge (Python, Apache-2.0; needs PostgreSQL and Redis; in-process plugins). Rejected: Docker MCP Gateway (spawns containers, conflicts with no Docker socket), MetaMCP (no per-user permissions or audit found), Microsoft MCP Gateway (Kubernetes, per-adapter access).

Needs Phase 4, G0, G9. Several agentgateway facts are unverified (stdio servers as subprocesses without Docker, HTTP proxy support for the egress proxy, JWT algorithms, memory footprint, release cadence), so P5A.1 is a spike that ends at G13. If the owner picks ContextForge, stop: rewrite P5A.3, P5A.4, P5A.7 and P5A.12 (PostgreSQL and Redis containers, plugins in place of parts of the front) and commit that rewrite before going on. No ADR defines the MCP gateway yet (ADR 0001 rule 3: a new image or service needs one), so nothing past P5A.2 starts until the owner accepts ADR 0057 (G9). Values introduced here (approval expiry 60 s, fetch size and time limits, the one-time ID window) go in `constants.rs`, with bounds in `askbar-proto` where a service checks them; the front gets them in config pushes (ADR 0028). Tool output is untrusted data everywhere. The MCP gateway holds only tool credentials, has no route to `/api/server/v1` or the admin API, and its egress is default deny per destination (ADR 0019). `search` and `fetch` are each a per-user permission, off by default.

- [ ] **P5A.1** `docs(docs): evaluate agentgateway as the MCP gateway` · sonnet · after Phase 4
  Files: `docs/mcp-gateway-evaluation.md`; a scratch compose file outside the repo (not committed; its content is quoted in the doc).
  Do: in a scratch compose, check each unverified item and record a yes, no or unknown with the command and output that show it: (1) a stdio MCP server runs as a subprocess without Docker or a Docker socket; (2) agentgateway reaches a tool server and an upstream through an HTTP proxy (the egress proxy from Phase 4); (3) which JWT algorithms it accepts for the user identity the front passes; (4) memory idle and under a short load; (5) release cadence, versions, image digest and architectures, licence file. Also confirm `mcpAuthorization` per-user per-tool CEL works and cannot read arguments. For the fallback, record ContextForge's footprint (PostgreSQL, Redis) and plugin hook points. Pin nothing yet.
  Accept: every item answered; the doc ends with a recommendation and the changes to the plan if ContextForge is picked; `scripts/check-docs.sh` passes. Gate: owner picks agentgateway or ContextForge (G13).
- [ ] **P5A.2** `docs(adr): propose 0057 MCP gateway and its front` · sonnet · after P5A.1, G13
  Files: `docs/adr/0057-mcp-gateway.md` (from `0000-template.md`), `docs/adr/README.md` index row.
  Do: Status Proposed. The component name (A12: not confusable, ADR 0021 rule 4, ADR 0038), the API audience's path prefix and OpenAPI file (`docs/openapi/mcp-gateway.v1.yaml`, ADR 0022), the split between the front and the chosen software, networks (the software on an internal network only, the front the one door), hardening per ADR 0032 (`read_only`, `cap_drop: ALL`, `no-new-privileges`, `pids_limit`, memory limit, tmpfs `/tmp`), image pinned by digest with licence notices (ADR 0017), holds only tool credentials, no route to `/api/server/v1` or the admin API, egress default deny per destination, untrusted tool output, the fetch rules, the fallback and the rejected options. Link the amendments in P5A.24 to P5A.29. Do not write "Accepted".
  Accept: ADR format check passes; README row added. Gate: owner accepts (G9).
- [ ] **P5.2** `feat(proto): add task, approval and decide types` · haiku · after P5A.2
  Files: `crates/askbar-proto/src/{task,approval,decide}.rs` (new), `lib.rs` (mod lines), tests.
  Do: `Task`, `TaskStatus` (`running`, `done`, `stopped`, `interrupted`), `Approval {action_hash, task_id, user, one_time_id, expires_at, signature}`, `ToolAction {tool, arguments, destination}`, `DecideRequest` and `DecideResponse` (use `judge`, `step_check`, `research`). Additive, `#[serde(default)]`.
  Accept: round-trip tests; old payloads without the fields parse; the hash of an action is stable across field order (canonical JSON).
- [ ] **P5.3** `feat(admin): create and push the approval signing key` · sonnet · after P5.2, G0, G9
  Files: `services/admin/src/{secrets,gateway_link}.rs`, tests, OpenAPI if a route is added.
  Do: Ed25519 key made by the admin, pushed to `model-gateway` and held in its memory only (like provider keys, ADR 0028); the public key is pushed to the MCP gateway front; rotation re-pushes both; never in logs, `/status`, audit attributes or any API response.
  Accept: tests: key absent from every output; rotation changes the public key version; a service before its first push signs and verifies nothing (fail closed).
- [ ] **P5.4** `feat(model-gateway): sign approvals` · sonnet · after P5.3
  Files: `services/model-gateway/src/{approval,lib}.rs` (new `approval.rs`), `tests/api.rs`.
  Do: sign the hash of the exact action, task, user, a one-time ID and a 60 s expiry (expiry from `constants.rs`, bounds in `askbar-proto`); record each approval in the audit span (hash and ID, no arguments). Signing is called by `/decide` in P5.6; here it is a function with its own tests.
  Accept: a changed argument changes the hash; expiry and one-time ID are present; no key in any output.
- [ ] **P5.5** `feat(proto): verify approvals` · sonnet · after P5.2
  Files: `crates/askbar-proto/src/approval.rs`, tests (or a new crate if the owner chose that at G9).
  Do: verify signature, action hash, expiry and one-time ID against the public key; the one-time ID store is a small trait the caller supplies; shared by the MCP gateway front and, in Phase 9, the executor. Every failure is a distinct error code.
  Accept: tests with a signed fixture: valid passes; unsigned, wrong key, swapped action, expired, replayed (second use) each fail with its own code.
- [ ] **P5A.3** `feat(compose): add the agentgateway image pinned by digest` · sonnet · after P5A.2, P5.5
  Files: `Dockerfile`, `compose.yaml` (build target only, not started yet), `scripts/third-party-notices.images.md`, `scripts/third-party-notices.sh`, `scripts/check-cpa-image.sh` pattern for a new `scripts/check-mcp-image.sh`.
  Do: a build stage that takes agentgateway by digest (ADR 0017), no tag-only reference; the licence text copied into the notices as for CLIProxyAPI; a check that the digest and version in the Dockerfile match the notices. New image: stop-and-ask until G9 is cleared.
  Accept: the check script passes; the notices name the version and licence (Apache-2.0); `cargo deny` untouched (no new crate).
- [ ] **P5A.4** `feat(compose): scan the agentgateway image for vulnerabilities` · sonnet · after P5A.3
  Files: `scripts/scan-third-party-images.sh` (new), `docs/mcp-gateway-evaluation.md` (a "scan" paragraph).
  Do: the Trivy `images` CI job (`.github/workflows/images.yml`) scans pushed digests with its flags and `.trivyignore.yaml`; first work out whether it covers a pulled third-party image such as agentgateway and `cpa`'s base (a gap if it does not). Add a local script that runs the same Trivy flags on every digest the Dockerfile pins from outside, and write the exact workflow change for the owner. The orchestrator never edits `.github/workflows/`.
  Accept: the script runs on the agentgateway image; the PR body states whether CI already covered it and lists the workflow change for the owner. Gate: the owner applies or declines the workflow change.
- [ ] **P5A.5** `feat(admin): add MCP gateway values to constants.rs` · haiku · after P5.2, G0
  Files: `crates/askbar-proto/src/mcp.rs` (new), `services/admin/src/constants.rs`, tests.
  Do: `MIN_`/`MAX_`/`DEFAULT_` for the fetch size limit, fetch time limit, tool-call time limit, the one-time ID window, and the maximum search URLs remembered per task; each with a doc comment; the approval expiry (60 s) is reused from P1.1a. Added to the struct served as `/admin/constants.json`.
  Accept: tests `MIN <= DEFAULT <= MAX`; the served JSON has no secret-looking keys; existing constants tests pass untouched.
- [ ] **P5A.6** `feat(mcp-gateway): add the front service with status and config push` · sonnet · after P5A.3, P5A.5
  Files: `services/mcp-gateway/{Cargo.toml,src/{main,lib,config,status}.rs,tests/api.rs}` (name per ADR 0057, A12), workspace `Cargo.toml`, `docs/openapi/mcp-gateway.v1.yaml`.
  Do: a Rust service built on `askbar-service`; a supervisor starts and restarts agentgateway as a child process (the `cpa-supervisor` pattern); `GET /status` shows the config version and the agentgateway state, no keys or arguments; `PUT /config` takes pushes and validates them against the `askbar-proto` bounds, keeping the old config on a bad push; before its first push it refuses every tool call (fail closed) and uses the `askbar-proto` defaults.
  Accept: tests: bad push leaves old config live; version hash changes with content; `/status` shows no secret; OpenAPI test passes.
- [ ] **P5A.7** `feat(mcp-gateway): generate agentgateway config from pushed config` · sonnet · after P5A.6
  Files: `services/mcp-gateway/src/agw_config.rs` (new), tests.
  Do: turn the pushed registry (servers, transports, credentials, per-user tool rules as CEL where agentgateway can express them) into the agentgateway config file in a tmpfs path; credentials only in that file and memory, never in logs, `/status` or errors; reload or restart agentgateway on a changed config and report the version it runs.
  Accept: golden-file tests for the generated config; a test greps every output for a marker credential and finds nothing; agentgateway listens on the internal address only (a test checks the generated listener address).
- [ ] **P5A.8** `feat(mcp-gateway): verify the signed approval before any tool call` · sonnet · after P5.5, P5A.6
  Files: `services/mcp-gateway/src/{approval,call}.rs`, tests.
  Do: `POST` call route takes the tool, exact arguments and the approval; recompute the action hash, run the P5.5 verify with the pushed public key and an in-memory one-time ID store (window from `constants.rs`); any failure refuses with `problem+json` (ADR 0023) and a distinct code, before anything reaches agentgateway.
  Accept: unsigned, replayed, swapped, expired and wrong-key approvals are each refused; a valid one passes to the next check.
- [ ] **P5A.9** `feat(mcp-gateway): write the audit span before each call` · sonnet · after P5A.8
  Files: `services/mcp-gateway/src/{audit,call}.rs`, `docs/style/logging-and-audit.md`.
  Do: through the Rust `AuditClient` open the span (user, task, tool, approval ID, argument hash, no argument text) and call only after the auditor acknowledges; the full fetch URL is recorded in its own span before a fetch (P5A.15); if the auditor does not acknowledge, refuse the call (fail closed); close the span with the outcome.
  Accept: a test with an unreachable auditor refuses the call and agentgateway sees nothing; a log grep for a marker argument finds nothing.
- [ ] **P5A.10** `feat(mcp-gateway): check per-user tool permissions` · sonnet · after P5A.9
  Files: `services/mcp-gateway/src/permissions.rs` (new), tests.
  Do: pushed config holds, per user, the permitted tools (`search` and `fetch` off by default, ADR 0019); the front refuses a call to a tool the user does not hold, and `GET` tools lists only the user's permitted tools; the same rule also goes into agentgateway's CEL where it can, as a second barrier.
  Accept: permission off refuses with its own code; a user with no entry holds nothing; the list shows only permitted tools.
- [ ] **P5A.11** `feat(mcp-gateway): list tools and forward approved calls to agentgateway` · sonnet · after P5A.7, P5A.10
  Files: `services/mcp-gateway/src/{tools,call}.rs`, `docs/openapi/mcp-gateway.v1.yaml`, tests with a mock MCP server.
  Do: `GET` tools returns the user's permitted tools (name and argument schema); the call route forwards a call that passed the approval, audit and permission checks to agentgateway over the internal address, applies the tool-call time limit and a response size limit (both from `constants.rs`), and returns the result marked as untrusted tool output.
  Accept: tests with a mock tool server: list, call, timeout, oversize reply; a call that failed any earlier check never reaches the mock.
- [ ] **P5A.12** `feat(compose): add the mcp-gateway service` · sonnet · after P5A.11, P5A.4
  Files: `compose.yaml`, `Dockerfile`, `scripts/check-isolation.sh`.
  Do: the front and agentgateway in one container; hardening per ADR 0032 (`read_only`, `cap_drop: ALL`, `no-new-privileges`, `pids_limit`, memory limit, tmpfs `/tmp`); the network from ADR 0057: reachable by `server` only, no route to `/api/server/v1` or the admin API, outbound only through the egress proxy; no published port. New container, network: stop-and-ask until G9.
  Accept: `scripts/check-isolation.sh` proves the isolation rules and that nothing but the front answers on the agentgateway port.
- [ ] **P5A.13** `feat(compose): add searxng` · haiku · after P5A.12
  Files: `compose.yaml`, `scripts/check-isolation.sh`, `scripts/third-party-notices.images.md`.
  Do: SearXNG image pinned by digest, notices and scan as in P5A.3 and P5A.4; hardened per ADR 0032; reachable only from the MCP gateway; outbound only through the egress proxy.
  Accept: isolation check passes; no published port.
- [ ] **P5A.14** `feat(mcp-gateway): add the search MCP server` · sonnet · after P5A.13, P5A.10
  Files: `services/mcp-gateway/src/search.rs` or an off-the-shelf SearXNG MCP server image (chosen by the spike; a new image or dependency is a stop-and-ask), `agw_config.rs`, tests.
  Do: register a `search` tool with agentgateway that queries searxng; the front remembers the URLs of each task's results (bounded count from `constants.rs`) for the fetch rules; the tool is off for every user until the admin grants it.
  Accept: a mock searxng returns results; results are marked untrusted; a user without the permission is refused. Gate: owner approves the server and its destinations (G14).
- [ ] **P5A.15** `feat(mcp-gateway): add the fetch MCP server and the ADR 0019 fetch rules` · sonnet · after P5A.14, P5A.9
  Files: `services/mcp-gateway/src/{fetch,fetch_rules}.rs`, tests.
  Do: a `fetch` tool; the front enforces ADR 0019's rules (read the ADR text first): fetch only a URL that came from this task's search results or from the user (the server marks it, and the signed action covers the URL, so the judge saw it); size and time limits from `constants.rs`; resolve the host and refuse private, loopback and link-local addresses; redirects re-checked; the full URL goes in the audit span before the fetch runs; the fetched page is untrusted data.
  Accept: unit tests per rule (unlisted URL, oversize, slow, redirect to a private address, private address by name and by literal); the span exists before the request. Gate: owner approves the server and its egress rule (G15).
- [ ] **P5A.16** `feat(admin): register MCP servers and their credentials` · sonnet · after P5A.6, G0, G16
  Files: `services/admin/src/{mcp,secrets,db}.rs` (new `mcp.rs`), one route line each in `lib.rs`, `docs/openapi/admin.v1.yaml`, tests.
  Do: append a migration for MCP servers (name, transport, address or command, enabled); an API to create, list, update and delete them; each credential is stored as provider keys are, sent only to the front and never returned; values and limits in `constants.rs`; each change is an admin action. Search and fetch are registered as built-in entries.
  Accept: a credential never appears in any response, log or audit attribute; an unregistered server cannot be called; OpenAPI test passes. Gate: each new server is a separate owner approval (G16).
- [ ] **P5A.17** `feat(admin): set per-user MCP tool permissions` · sonnet · after P5A.16
  Files: `services/admin/src/mcp.rs`, `db.rs` migration, OpenAPI, tests.
  Do: per user and tool: allow or deny, default off for every tool (ADR 0019); an API to read and set them; each change is an admin action; deleted with the user.
  Accept: a user with no row holds nothing; set and read round-trip; user-delete test removes rows.
- [ ] **P5A.18** `feat(admin): push MCP config to the front` · sonnet · after P5A.17, P5A.7, P5.3
  Files: `services/admin/src/mcp_link.rs` (new), tests.
  Do: push registry, credentials, permissions, the constants it uses and the approval public key to the front's `PUT /config`, re-pushing on a version mismatch at the interval in `constants.rs` (ADR 0028); a rejected push shows its error and does not loop faster than the interval.
  Accept: a fake front receives the push; a changed permission changes the version; an old front that ignores new fields still accepts.
- [ ] **P5A.19** `feat(admin): push egress allowlist entries per MCP server destination` · sonnet · after P5A.18, P4.4
  Files: `services/admin/src/{mcp,egress_link}.rs`, tests.
  Do: for each enabled MCP server, one allowlist entry per destination, sourced to the `mcp-gateway`, through the mechanism P4.4 built; nothing outside the registry is allowed; a server registered without a destination has no egress. Each new destination is an owner gate (G14, G15, G16).
  Accept: tests: a registered destination appears in the push, a removed one disappears; a private address is refused unless the owner approved it as a host.
- [ ] **P5A.20** `test(mcp-gateway): refuse unsigned, replayed, swapped and expired approvals` · sonnet · after P5A.8
  Files: `services/mcp-gateway/tests/approvals.rs`.
  Do: ADR 0016 "Defend": a signed approval from the real signer path (P5.4) is replayed, its arguments swapped, its expiry passed, its signature stripped, and signed with another key.
  Accept: each is refused before any call reaches the mock tool server, and each is recorded.
- [ ] **P5A.21** `test(mcp-gateway): refuse the call when the audit write fails or the permission is off` · sonnet · after P5A.9, P5A.10
  Files: `services/mcp-gateway/tests/{audit,permissions}.rs`.
  Do: an auditor that is down, slow or answers an error; a permitted and a not-permitted user; a permission removed by a config push between two calls.
  Accept: the call is refused in each failing case and reaches the mock tool server in none; the allowed case passes.
- [ ] **P5A.22** `test(mcp-gateway): enforce the fetch rules and block private addresses` · sonnet · after P5A.15
  Files: `services/mcp-gateway/tests/fetch.rs`.
  Do: URLs not from search results or the user, oversize and slow responses, redirects, private, loopback and link-local addresses, a host that resolves to one, and a literal IP.
  Accept: each refused; the full URL appears in the audit span before every allowed fetch.
- [ ] **P5A.23** `test(mcp-gateway): search and fetch end to end` · haiku · after P5A.19, P5A.22
  Files: `scripts/smoke.sh`, `tests/e2e/` fixtures.
  Do: smoke step with a mock searxng and a mock page: sign an approval, search, fetch a result, check the audit spans and that an unsigned call and a direct request to the agentgateway port are refused.
  Accept: `scripts/smoke.sh` passes.
- [ ] **P5A.24** `docs(adr): amend 0019: the MCP gateway is agentgateway behind a front` · haiku · after P5A.15. Files: `0019-post-v1-order-and-tools.md`. Order: routing, egress proxy, MCP gateway, research and judge, agents with MCP tools, admin UI, executor, library; the fetch rules are enforced in the front. Decision.
- [ ] **P5A.25** `docs(adr): amend 0028: MCP gateway config and the approval public key` · haiku · after P5A.18. Files: `0028-service-boundaries-and-config-delivery.md`. The front takes config pushes with constants, permissions and the approval public key; tool credentials are pushed and held in memory. Decision.
- [ ] **P5A.26** `docs(adr): amend 0032: MCP gateway and searxng container limits` · haiku · after P5A.13. Files: `0032-container-hardening.md`.
- [ ] **P5A.27** `docs(adr): amend 0017: a third-party image and its licence` · haiku · after P5A.3. Files: `0017-dependencies-and-patches.md`. Third-party images pinned by digest with licence notices; agentgateway Apache-2.0.
- [ ] **P5A.28** `docs(adr): amend 0026: scan third-party images` · haiku · after P5A.4. Files: `0026-dependency-license-check.md`. The image scan and the gap found in P5A.4.
- [ ] **P5A.29** `docs(adr): amend 0016: tool servers as an attack surface` · haiku · after P5A.22. Files: `0016-risk-register.md`. Defend: unsigned, replayed, swapped or expired approvals refused, audit failure refuses, permission off refuses, fetch rules and private addresses (tests P5A.20 to P5A.22). Accept: a poisoned tool server or page can return hostile output; agentgateway's maturity and unverified behaviour; a compromised front can call any tool the gateway holds credentials for. Register.

Phase end: full checks, PR. The orchestrator opens it for the owner; G14, G15 and G16 stay open until each destination is approved.

## Phase 5B: research reports and the judge (branch `feat/research-judge`, coarse)

Expand into detailed steps at phase start. Needs Phase 5A. Values introduced here (task tool-call default 8 and range 1 to 32, research waits 8 s and 90 s, summary size 800 tokens, decider and judge thresholds) go in `constants.rs`, with bounds in `askbar-proto` where a service checks them (approval expiry is already in Phase 5A); the MCP gateway gets its values through the config push built in P5A.18.

- **P5.1 to P5.5, P5.11, P5.12, P5.16** moved to 5A: P5.1 became P5A.2 (ADR 0057); P5.2 to P5.5 keep their IDs there; P5.11 is P5A.6 to P5A.12; P5.12 is P5A.13; P5.16 is P5A.20.
- [ ] **P5.6** `feat(model-gateway): add POST /decide for the judge role` · sonnet · fixed input, fixed output format, anything else refuses; pinned model and backup.
- [ ] **P5.7** `feat(model-gateway): run the decider cascade for the judge in shadow` · sonnet · thresholds from labeled tool actions.
- [ ] **P5.8** `feat(model-gateway): add step_check and research decisions to /decide` · sonnet
- [ ] **P5.9** `feat(server): add task tables and mark running tasks interrupted on start` · sonnet · migration; ADR 0009.
- [ ] **P5.10** `feat(server): enforce the tool-call limit and task cost cap` · sonnet · 402 `problem+json`; alerts at 75% and 90%; default and range (8; 1 to 32) from `constants.rs`.
- [ ] **P5.13** `feat(server): run research reports` · sonnet · split, researchers, wait rules (8 s, 90 s), summary size (800 tokens), writer, degrade; values from `constants.rs`.
- [ ] **P5.14** `feat(model-gateway): add role pins and the role table` · sonnet
- [ ] **P5.15** `feat(admin): set roles, decider thresholds and task caps` · sonnet · within the `constants.rs` ranges, pushed with each service's config
- [ ] **P5.17** `docs(adr): amend 0004 and 0049, 0023` · haiku · one commit each: new tokens and volumes and the signing key, new problem types (task cap 402, bad approval). (0028 and 0032 are amended in P5A.25 and P5A.26.)
- [ ] **P5.18** `docs(adr): amend 0054, 0039, 0040 for research and the judge` · haiku · one commit each.

## Phase 6: agents with MCP tools (branch `feat/agents`)

Agents use MCP tools only; there is no executor and no G10 (the executor is Phase 9). Needs Phase 5A, Phase 5B and G0. It reuses Phase 5B's task tables (P5.9), tool-call limit and cost cap (P5.10), `/decide` (P5.6, P5.8) and role table (P5.14); do not rebuild them. Every tool call goes `server` to `/decide` (use `judge`) to a signed approval to the MCP gateway front; tool output is untrusted data and never reaches the judge. New behaviour values (maximum steps per plan, the fixer round limit) go in `constants.rs` with bounds in `askbar-proto`.

- [ ] **P6.1** `feat(proto): add plan and step types` · haiku · after P5.2
  Files: `crates/askbar-proto/src/task.rs`, tests.
  Do: `Plan {steps}`, `PlanStep {id, instructions, inputs, tool: Option<String>, check}` with at most one allowed MCP tool or none, and `Check` as `schema {json_schema}`, `decider` or `planner`; the planner's output JSON schema; additive, `#[serde(default)]`.
  Accept: round-trip tests; a step with two tools does not parse; old task payloads still parse.
- [ ] **P6.2** `feat(admin): add agent values to constants.rs and push them to the server` · sonnet · after P6.1, P5.15, G0
  Files: `crates/askbar-proto/src/task.rs`, `services/admin/src/{constants,server_link}.rs`, `services/server/src/snapshot.rs`, tests.
  Do: `MIN_`/`MAX_`/`DEFAULT_` for the maximum steps per plan, the fixer round limit (1: a second fixer round ends the task), the largest tool output kept in a worker's context; the tool-call and cost-cap values from P5.10 are reused. Pushed in the server snapshot, additive and defaulted, rejected outside the bounds.
  Accept: a changed value changes the snapshot version; an out-of-bounds value is rejected and the old one stays; an old snapshot parses.
- [ ] **P6.3** `feat(server): read the user's permitted MCP tools through the front` · sonnet · after P5A.11, P5A.17, P6.1
  Files: `services/server/src/mcp.rs` (new), `docs/openapi/mcp-gateway.v1.yaml` (client side), tests with the mock MCP front.
  Do: at task start the server asks the MCP gateway front for the user's permitted tools (names and argument schemas); the answer is the only tool list the planner sees; tool descriptions are untrusted text (length-capped, never treated as instructions); an unreachable front means a task with no tools, recorded.
  Accept: a user with no permission gets an empty list; the list is read per task, not cached across users.
- [ ] **P6.4** `feat(server): ask the planner for a plan limited to the permitted tools` · sonnet · after P6.3, P5.14
  Files: `services/server/src/agent/{planner,mod}.rs` (new), tests.
  Do: one `agent.planner` call through `model-gateway` (`role`, `task_id`) with the user's request and the permitted tool names and schemas; a fixed prompt asks for the P6.1 plan JSON and nothing else; the step row for the plan is written before the call.
  Accept: a scripted mock plan parses; a reply that is not the plan format is an invalid plan (P6.5).
- [ ] **P6.5** `feat(server): validate the plan before any step runs` · sonnet · after P6.4, P5.10
  Files: `services/server/src/agent/validate.rs` (new), tests.
  Do: refuse a plan if a tool does not exist, is not in the user's permitted list, a step names more than one tool, the step count passes the maximum, the plan's possible tool calls pass the task tool-call limit (P5.10), a `schema` check has an invalid JSON schema, or a `decider` check is chosen while no decider is set for `step_check` (it becomes `planner`); A13 applies to a refused plan.
  Accept: table tests per rule; a second invalid plan stops the task with its error codes recorded.
- [ ] **P6.6** `feat(server): run a worker step with its step context only` · sonnet · after P6.5, P5.9
  Files: `services/server/src/agent/worker.rs` (new), tests.
  Do: write the step's `task_steps` row before it starts (P5.9); one `agent.worker` call with the step's instructions, inputs, the outputs it needs from earlier steps and the one allowed tool's schema, and nothing else; the reply is a tool call request or a final output; loop until a final output; each attempt is its own span linked by `task_id` and `decision_id`.
  Accept: a test inspects the request body: no other step's text, no plan, no other tool; the row exists before the call.
- [ ] **P6.7** `feat(server): send each tool call through the judge, a signed approval and the MCP front` · sonnet · after P6.6, P5.6, P5A.11, P5.10
  Files: `services/server/src/agent/tool_call.rs` (new), tests.
  Do: refuse a call to a tool other than the step's allowed tool; apply ADR 0019's fixed rules; check and count the tool-call limit before the call; `POST /decide` with use `judge`, the tool, exact arguments and destination, the user's original request and the plan step, never fetched pages or tool output; on approval call the MCP front with the signed approval; on a refusal or a missing approval the call does not happen and the worker gets only the reason code; the tool result goes to the worker as a delimited block of untrusted data, cut to the context limit from P6.2.
  Accept: with a mock `/decide` and a mock front: approved call runs once; refusal runs nothing; an unplanned tool is refused before `/decide`; the judge input in the request body has no tool output.
- [ ] **P6.8** `feat(server): check steps against a JSON schema` · sonnet · after P6.6
  Files: `services/server/src/agent/check.rs` (new), tests.
  Do: validate the step's final JSON output against the step's schema (A13); if no JSON schema crate is in `Cargo.lock`, stop and ask before adding one (new dependency; `cargo deny`).
  Accept: valid passes, invalid and non-JSON fail with distinct codes.
- [ ] **P6.9** `feat(server): review a step with the planner when no deterministic check applies` · sonnet · after P6.8
  Files: `services/server/src/agent/check.rs`, tests.
  Do: a `planner` check, and the fallback when a decider check is below `pass`, unset or errors, calls `agent.planner` with the step's acceptance text and output in a fixed pass or fail format; any other reply is a fail.
  Accept: scripted pass, fail and off-format replies behave as stated.
- [ ] **P6.10** `feat(server): check steps with the decider's pass answer` · sonnet · after P6.9, P5.8
  Files: `services/server/src/agent/check.rs`, tests.
  Do: for a `decider` check call `/decide` with use `step_check`; at or above `pass` the step is done, below it the planner reviews (P6.9); a decider error goes to the planner; live only when `step_check` mode is `live` (shadow first, P6.18); the decider never sees tool output beyond the step's output text.
  Accept: tests for at, above and below the threshold, decider error and shadow mode (planner decides).
- [ ] **P6.11** `feat(server): retry once on standard, then the fixer` · sonnet · after P6.9, P6.2
  Files: `services/server/src/agent/{retry,fixer}.rs` (new), tests.
  Do: a failed check reruns the step once on `standard` (`worker_retry` in `reasons`); a second failure calls `agent.fixer`, which returns revised steps that go through P6.5 validation; a second fixer round on one step ends the task as `stopped` with results saved; every attempt is a row.
  Accept: scripted failures: one retry then pass; retry then fixer then pass; second fixer round stops the task; revised steps with a new tool are refused.
- [ ] **P6.12** `feat(server): stop at the tool-call limit or cost cap and answer from finished work` · sonnet · after P6.7, P6.11, P5.10
  Files: `services/server/src/agent/{finish,mod}.rs`, tests.
  Do: use P5.10's checks (tool-call limit; cost cap with `problem+json` 402 and alerts at 75% and 90%); when either stops the task the planner answers from finished steps only; a finished plan ends with the planner's final answer; status `done` or `stopped`; tiers do not change mid-task.
  Accept: limit and cap tests stop mid-plan with saved results and a planner answer; a normal run ends `done`.
- [ ] **P6.13** `feat(server): mark provider failures interrupted and resume` · sonnet · after P6.11, P5.9
  Files: `services/server/src/agent/resume.rs` (new), `store.rs`, tests.
  Do: when every in-tier fallback try fails the task ends `interrupted`; Resume continues from the next unfinished step; a step whose row started but never ended reruns only after the user confirms (its tool calls may have happened); the start-up sweep that marks `running` tasks `interrupted` (P5.9) is reused.
  Accept: tests: resume skips finished steps; a cut-off step without confirmation does not run; with confirmation it reruns once; a restart marks a running task interrupted.
- [ ] **P6.14** `feat(server): add the client API to start an agent task and stream progress` · sonnet · after P6.12
  Files: `services/server/src/*` (routes), `docs/openapi/client.v1.yaml`, `docs/wire-contract.md`, tests.
  Do: follow "Adding an endpoint, end to end": start a task for a conversation, stream progress (plan, step started, tool call refused or run, check result, retry, done) as SSE events with no tool output text, read a task's state; 402 `problem+json` for the cap; additive.
  Accept: OpenAPI test; old clients ignore the new events; a task of another user is 404.
- [ ] **P6.15** `feat(server): add the client API to resume and cancel a task` · sonnet · after P6.13, P6.14
  Files: same as P6.14.
  Do: Resume with an optional `confirm_rerun` for the cut-off step; cancel stops the loop after the current call, keeps results and marks the task `stopped`.
  Accept: OpenAPI test; resume without confirmation of a cut-off step is refused with a clear error; cancel is idempotent.
- [ ] **P6.16** `feat(model-gateway): add planner, worker and fixer role defaults` · haiku · after P5.14
  Files: `services/model-gateway/src/routing.rs`, tests.
  Do: `agent.planner` `frontier`; `agent.worker` `fast` with `retry_tier` `standard`; `agent.fixer` `frontier`; a pin replaces the tier, with a backup pin.
  Accept: tests per role; a pin to an unapproved model rejects the push.
- [ ] **P6.17** `feat(admin): set pins for the planner, worker and fixer` · sonnet · after P6.16, P5.15, G0
  Files: `services/admin/src/{routing,gateway_link}.rs`, OpenAPI, tests.
  Do: API to read and set the three roles' tiers and pins within the `constants.rs` ranges; push to the gateway; warn when the judge's pinned provider equals the planner's or worker's.
  Accept: a pin to an unapproved model is refused before pushing; the warning appears.
- [ ] **P6.18** `feat(model-gateway): shadow decider step checks against the planner` · sonnet · after P5.8, P6.10
  Files: `services/model-gateway/src/{decide,decider}.rs`, tests.
  Do: while `step_check` mode is `shadow` the planner's verdict decides and the decider's `pass` probability is recorded beside it in the decision record (probabilities only); the decider call opens its own span first and counts against the task cap.
  Accept: no step outcome changes in shadow; both answers are on the record; a decider failure records `decider_error`.
- [ ] **P6.19** `feat(admin): report step-check shadow results` · sonnet · after P6.18, P1.6, G0
  Files: `services/admin/src/routing.rs`, OpenAPI, tests.
  Do: `GET /api/admin/v1/routing/step-check-report`: per decider, the share of steps where its pass or fail agreed with the planner, errors and timeouts, from decision records only; suggested `pass` threshold.
  Accept: fixture records give expected numbers; the owner reads this before flipping `step_check` live.
- [ ] **P6.20** `feat(client): show agent tasks and progress` · sonnet · after P6.14
  Files: `clients/desktop/ui/*`, `clients/desktop/src/*`.
  Do: a task view in the conversation: plan steps with status, tool calls used against the limit, cost against the cap, retries; text via `textContent`; follow `docs/style/ui-ux.md`.
  Accept: client checks and `npm test`.
- [ ] **P6.21** `feat(client): add Resume, Cancel and the rerun confirmation` · sonnet · after P6.15, P6.20
  Files: `clients/desktop/ui/*`, `clients/desktop/src/*`.
  Do: Resume and Cancel on an interrupted or running task; a dialog before rerunning a cut-off step that says its tool calls may already have happened.
  Accept: UI tests for resume, cancel and both answers to the dialog.
- [ ] **P6.22** `test(e2e): add a mock MCP tool server` · haiku · after P5A.11
  Files: `tests/e2e/mock_mcp.py` (new), `compose.yaml` `dev` profile entry if needed (stop-and-ask for a container).
  Do: an MCP tool server with tools `echo`, `search` and `fetch` and keywords: slow, 500, oversize, and output that carries hostile text.
  Accept: usable by P6.24 and P6.25.
- [ ] **P6.23** `test(e2e): script mock LLM plans` · haiku · after P6.1
  Files: `tests/e2e/mock_llm.py`.
  Do: keyword-selected scripted planner plans (valid, invalid tool, too many steps), worker replies (tool call, final output, bad JSON) and fixer revisions.
  Accept: usable by P6.24 and P6.25.
- [ ] **P6.24** `test(server): run agent tasks against the mock tool server and scripted plans` · sonnet · after P6.15, P6.22, P6.23
  Files: `services/server/tests/agents.rs`.
  Do: a two-step plan with a tool call (approved, signed, run once); schema check pass and fail; retry then fixer; second fixer round ends the task; tool-call limit and cost cap stops; provider failure to `interrupted`, Resume, and the rerun confirmation.
  Accept: every case ends in the stated task status with saved results.
- [ ] **P6.25** `test(server): tool output that asks for other tools or settings is refused` · sonnet · after P6.24
  Files: `services/server/tests/agents_injection.rs`.
  Do: ADR 0016 "Defend": the mock tool returns text telling the worker to call another tool, call a tool the user lacks, approve itself, or change settings; the scripted worker obeys it.
  Accept: the call to a tool outside the step is refused before `/decide`; a call to a tool the user lacks is refused by plan validation or the front; a mock judge refuses the crafted action; no setting changes; the judge request body holds no tool output.
- [ ] **P6.26** `test(server): agent task smoke` · haiku · after P6.24
  Files: `scripts/smoke.sh`.
  Do: smoke step with the mock LLM and mock tool server: start a task, watch progress, finish `done`.
  Accept: `scripts/smoke.sh` passes.
- [ ] **P6.27** `docs(adr): amend 0019: agents use MCP tools, executor deferred` · haiku · after P6.12. Files: `0019-post-v1-order-and-tools.md`. Decision.
- [ ] **P6.28** `docs(adr): amend 0054: agent roles and step checks` · haiku · after P6.18. Files: `0054-model-gateway.md`.
- [ ] **P6.29** `docs(adr): amend 0039: the agent loop and task API` · haiku · after P6.15. Files: `0039-server.md`.
- [ ] **P6.30** `docs(adr): amend 0040: agent role pins and step-check report` · haiku · after P6.19. Files: `0040-admin.md`.
- [ ] **P6.31** `docs(adr): amend 0044: agent tasks, Resume and rerun confirmation` · haiku · after P6.21. Files: `0044-desktop-client.md`.

Phase end: full checks, PR. The owner reads the step-check report before `step_check` goes live.

## Phase 7

Moved: the script library is Phase 10.

## Phase 8: admin web UI (branch `feat/admin-routing-ui`, deferred)

May start any time after G0 (G12 is the same gate); numbered after Phase 6, with Phases 9 and 10 (deferrable) behind it. Expand at phase start; follow the admin redesign plan's rules and its test suite (`tests/e2e/admin.test.js`).

- **P8.1** removed: constants live in `constants.rs` from Phase 1 (G0 merges the admin UI first, so nothing is left to move or rebase).
- [ ] **P8.2** `feat(admin): add the routing page` · sonnet · models, tiers, weights, prices, effort, pins, cutoffs, ceilings, caps, training share.
- [ ] **P8.3** `feat(admin): add decider settings` · sonnet · providers, thresholds, zero-retention record.
- [ ] **P8.4** `feat(admin): add the dry-run panel and decider comparison` · sonnet
- [ ] **P8.5** `feat(admin): add example labeling and shared-question review` · sonnet
- [ ] **P8.6** `feat(admin): add spend and subscription limit views` · sonnet
- [ ] **P8.6a** `feat(admin): add MCP server and tool permission pages` · sonnet · after P5A.17 · register servers and credentials (write-only fields), per-user tool permissions, each destination's egress status.
- [ ] **P8.6b** `feat(admin): add agent task views` · sonnet · after P6.17 · task list with status, tool calls used, cost against the cap, steps and decision links; no step content or tool output.
- [ ] **P8.7** `feat(admin): add script review` · sonnet · after P10.4 · only when Phase 10 ships.
- [ ] **P8.8** `test(admin): cover the routing, MCP and task pages at 1280 and 390` · haiku
- [ ] **P8.9** `docs(docs): update the admin guide` · haiku

## Phase 9: executor (branch `feat/executor`, deferrable, coarse)

Deferrable: starts only when the owner asks, after Phase 8. Expand at phase start. Needs Phase 6, G10. The executor runs untrusted code; search and fetch stay on the MCP gateway. It verifies approvals with P5.5's code, has no `model-gateway` link and no secrets.

- [ ] **P9.1** `feat(proto): add executor work and output types` · haiku
- [ ] **P9.2** `feat(executor): add the service, pairing and approval check` · sonnet · public key at pairing, no gateway link, no secrets; G10.
- [ ] **P9.3** `feat(compose): add executor-link` · sonnet · optional Tailscale; `check-isolation.sh`.
- [ ] **P9.4** `feat(server): add code and file tools that run in the executor` · sonnet · planner tools beside the MCP tools; each call through the judge and a signed approval.
- [ ] **P9.5** `feat(server): check steps by exit code and tests` · sonnet · deterministic checks beside the schema check (P6.8).
- [ ] **P9.6** `docs(adr): amend 0020, 0054, 0039, 0040 for the executor` · haiku · one commit each; 0020 records the executor as built after the MCP gateway.

## Phase 10: script library (branch `feat/script-library`, deferrable, coarse)

Deferrable: starts only when the owner asks. Expand at phase start. Needs Phase 9, G11. Library thresholds (`match_min`, `fit_min`, `candidates`, `demote_below`, `retire_after_failures`) go in `constants.rs`, bounds in `askbar-proto`.

- [ ] **P10.1** `feat(server): add the script catalog and entry format` · sonnet · folder with `SCRIPT.md`, `inputs.json`, `run.*`, `tests/`, `manifest.yaml`; hash.
- [ ] **P10.2** `feat(executor): run tests for a draft in a sandbox` · sonnet · no network beyond the manifest's hosts.
- [ ] **P10.3** `feat(server): author drafts from finished tasks` · sonnet · `library.author` never sees fetched pages or tool output.
- [ ] **P10.4** `feat(admin): review, promote, edit and reject scripts (API)` · sonnet · only the owner promotes; hash pinned.
- [ ] **P10.5** `feat(model-gateway): match requests to scripts in shadow` · sonnet · decider `choice` over all entries (hosted Jev up to 255 options per question, self-hosted passes of 52) with a keyword prefilter above that, then `noul`; no embeddings.
- [ ] **P10.6** `feat(server): fill inputs and validate against the schema` · sonnet
- [ ] **P10.7** `feat(server): run a matched script with a signed approval` · sonnet · dry-run first for writing scripts.
- [ ] **P10.8** `feat(server): track success, demote and retire scripts` · sonnet
- [ ] **P10.9** `test(server): unpromoted or changed scripts do not run` · sonnet · ADR 0016 "Defend".
- [ ] **P10.10** `docs(adr): amend 0016, 0054, 0039, 0040 for the library` · haiku · one commit each.

## Risks and open questions for the owner

- **Decider outage or latency.** Every Auto chat depends on it; failures go to `default_tier`; measure p95 per provider in shadow.
- **Hosted decider before the egress proxy.** The gateway reaches the internet directly until Phase 4. A hosted decider would run in shadow over that route, with the zero-retention record in place (G4). Alternative: a self-hosted decider only until Phase 4.
- **Self-hosted decider speed and licence.** OpenJev and Clef (both 27B) take seconds on a Mac mini M4 (OpenJev measured at 3 to 20 s); Clef-flash (9B) should take roughly a third of that (estimate, to be measured), still too slow for an Auto chat's latency budget unless measured fast enough, so a Mac fits shadow, judge pre-checks and local-only conversations. OpenJev's CC BY-NC weights need a commercial licence for this use; Clef and Clef-flash weights: check licence.
- **Decider maturity.** Jev is early access with no published p95 and overconfident probabilities; one benchmark reports Clef's calibration error at 0.02 to 0.04 on four of five datasets. Thresholds come only from Askbar's labeled sets, per decider, and a version change re-runs the check.
- **Undocumented usage addresses.** The two subscription usage endpoints may change; failures show "unknown" and never block calls.
- **agentgateway's unverified items and maturity.** Running stdio servers without Docker, HTTP proxy support for the egress proxy, JWT algorithms, memory footprint and release cadence are unchecked until the P5A.1 spike; it is a young Linux Foundation project, so a breaking release or a gap found late could force the ContextForge fallback (heavier: PostgreSQL, Redis) and a rewrite of P5A.3 to P5A.7.
- **MCP tool servers are a new attack surface.** Each server holds credentials and returns untrusted output. Defences: signed approvals, per-user permissions off by default, audit before the call, fetch rules, egress default deny per destination, an owner gate per destination (G14 to G16).
- **Third-party image licence and CVE scan gap.** agentgateway and searxng are pulled images; the Trivy `images` job may not scan them and each needs a licence notice (P5A.3, P5A.4); a CI change is the owner's.
- **Executor hardware.** Deferred with Phase 9; open only if the owner starts it (G10).
- **Training data.** 300 labels is the owner's work; shadow and the check report are not meaningful before it. Who labels, and when?
- **Dollar budgets.** The owner must enter dollar amounts and per-model prices before switching the unit; token budgets stay until then.
- **Assumptions A1 to A13** above need a yes or a change before the steps that use them.
