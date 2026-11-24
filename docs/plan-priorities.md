# Plan: order of work after 1.0

Previous: [Refactor](plan-refactor.md) · Next: [Audit viewer](plan-audit-viewer.md) · Tracker: [task-tracker.md](task-tracker.md)

Status: proposed, 2026-11-23; owner decisions added 2026-11-24. The order below is the owner's. The owner accepted the amendment to [ADR 0019](adr/0019-post-v1-order-and-tools.md) in step O0.1 on 2026-11-24, so the order is in effect. Rules for agents: [AGENTS.md](../AGENTS.md). How every plan runs: [orchestration.md](orchestration.md).

This file decides **order** and the few things the order changes. Each track's own plan decides behaviour and holds the steps. Where a track reuses steps from another plan, this file names them by ID instead of copying them.

## Order

| # | Track | Plan | Delivers | Status |
| --- | --- | --- | --- | --- |
| 0 | Refactor | [plan-refactor.md](plan-refactor.md) | Less duplicated code, before track 1 adds three services, new links and new checks on top of it. No behaviour change | done |
| 1 | Web search | this file, then the [tiered routing](plan-tiered-routing.md) and [audit viewer](plan-audit-viewer.md) steps it names | Step records, search and fetch through the MCP gateway, quick search in chat, then research reports and the judge | in progress |
| 2 | PII detection and redaction | [plan-pii-redaction.md](plan-pii-redaction.md) | Placeholders out, real values back, blocked secrets, PII checks on search queries | not started |
| 3 | Context checks and canaries | [plan-canaries.md](plan-canaries.md) | Stop reasons, context limits, the echo canary, leak tripwire, injection markers on fetched pages | not started |
| 4 | Tiered routing | [plan-tiered-routing.md](plan-tiered-routing.md) | Tiers, the decider, Auto, dollar budgets, then agents with MCP tools at the end | not started |
| 5 | Audit viewing | [plan-audit-viewer.md](plan-audit-viewer.md) | Conversation view of sealed records | not started |

A track starts when the one before it has its PR open, unless the owner says otherwise. Tracks 0 and 1 interleave: each track 1 stage starts when the refactor stage that clears its code has its PR open:

| Refactor stage | Track 1 stage that waits for it |
| --- | --- |
| R1 | W1 |
| R2 | W2 |
| R3 | W3 |
| R4 | W4.2 (and W2, if P4.1c adds a link first) |
| R5 | W5 |
| RL | Track 3 |

W0 is docs only and runs alongside R1.

## Plan index

Every plan in reading order. Each carries a `Previous / Next / Tracker` line at the top; [task-tracker.md](task-tracker.md) lists every step. A plan used by several tracks sits where its first step runs, and the tiered routing phases keep their phase order, so Phases 4, 5A, 5B and 8 (track 1) come after Phases 0 to 3 (track 4).

| # | Plan | Used by | Status |
| --- | --- | --- | --- |
| 1 | [plan-refactor.md](plan-refactor.md) | track 0 | done |
| 2 | [plan-priorities.md](plan-priorities.md) (this file: O0, W4, W5) | track 1 | in progress |
| 3 | [plan-audit-viewer.md](plan-audit-viewer.md) | W1, then track 5 | in progress |
| 4 | [plan-tiered-routing.md](plan-tiered-routing.md) (overview, gates) | tracks 1 and 4 | in progress |
| 5 | [Phase 0: records and docs](plan-tiered-routing/phase-0-docs.md) | track 4 | in progress |
| 6 | [Phase 1: records](plan-tiered-routing/phase-1-records.md) | track 4 | not started |
| 7 | [Phase 2: shadow](plan-tiered-routing/phase-2-shadow.md) | track 4 | not started |
| 8 | [Phase 3: Auto](plan-tiered-routing/phase-3-auto.md) | track 4 | not started |
| 9 | [Phase 4: egress proxy](plan-tiered-routing/phase-4-egress-proxy.md) | W2 | in progress |
| 10 | [Phase 5A: MCP gateway](plan-tiered-routing/phase-5a-mcp-gateway.md) | W3 | in progress |
| 11 | [Phase 5B: research and judge](plan-tiered-routing/phase-5b-research-judge.md) | W7, then track 4 | not started |
| 12 | [Phase 6: agents](plan-tiered-routing/phase-6-agents.md) | end of track 4 | not started |
| 13 | [Phase 8: admin UI](plan-tiered-routing/phase-8-admin-ui.md) | W6, then track 4 | not started |
| 14 | [Phase 9: executor](plan-tiered-routing/phase-9-executor.md) | deferrable | not started |
| 15 | [Phase 10: script library](plan-tiered-routing/phase-10-script-library.md) | deferrable | not started |
| 16 | [Risks and open questions](plan-tiered-routing/risks.md) | owner reading, no steps | not started |
| 17 | [plan-pii-redaction.md](plan-pii-redaction.md) | track 2 | not started |
| 18 | [plan-canaries.md](plan-canaries.md) | track 3 | not started |
| 19 | [plan-post-v1.md](plan-post-v1.md) | V2 sketch, no steps | not started |
| 20 | [plan-container-hardening.md](plan-container-hardening.md) | proposals, no steps | not started |
| 21 | [plan-single-image.md](plan-single-image.md) | proposed, no steps (G-R2) | not started |
| 22 | [plan-release-branches.md](plan-release-branches.md) | proposed, no steps | not started |
| 23 | [plan-admin-redesign.md](plan-admin-redesign.md) | finished, kept as the redesign record | done |

Not plans, so outside the order: [orchestration.md](orchestration.md) (how every plan runs) and [design-tiered-routing.md](design-tiered-routing.md) (the design the tiered routing plan cites; it stays in `docs/` beside it).

## How to run

Run every track as [orchestration.md](orchestration.md) says. Start one orchestrator session per track, on Sonnet:

> You are the orchestrator for track N of `docs/plan-priorities.md`. Read `docs/orchestration.md` and the track's plan file, then follow it from the first unchecked step.

Track 1's steps live in several plans. For a step named by ID (for example AV2.1 or P4.2), find it with `grep -n '\*\*AV2.1\*\*' docs/*.md docs/plan-tiered-routing/*.md` and read only that step's block, plus the plan's How to run section if it has one. Tick the box where the step lives.

## Owner decisions (2026-11-24)

1. **Step records come first.** The audit viewer's `step` span kind and payload roles move to the start of track 1, so every track records in its final shape. Steps are written by `model-gateway` only, as children of an attempt. W1 is AV0.2, AV2.1, AV2.2 and AV2.3 as reduced in the audit viewer plan. Track 5 keeps the conversation view.
2. **Quick search ships before research reports.** It is a small path of its own, not a one-question research report. Research reports and the judge follow it in track 1.
3. **Agents with MCP tools stay at the end of track 4.** No web search step depends on them: Phases 4, 5A and 5B name no Phase 6 step, and quick search uses no planner or worker role.

## Why this order

1. **Web search first.** It is the feature the family wants most, and ADR 0019 already makes it the first work after v1. It needs no routing: until tiers exist, each role uses a model the admin pins.
2. **PII second.** Web search opens a new path out of the house: search queries go to public search engines through `searxng`. PII checks on queries and prompts close that path and the provider path together.
3. **Canaries third.** Fetched pages make contexts much longer, so silent truncation becomes likely once search ships. Fetched pages are also where prompt injections arrive, so injection markers become useful now.
4. **Tiered routing fourth.** It saves money rather than adding a capability. Its decider thresholds need labeled examples, which real use, including search, produces in the meantime.
5. **Audit viewing last.** The records it shows exist from track 1 on, in their final shape. Until then the current per-item sealed view still works.

## Track 1: web search

Starts only after 1.0 ships: [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md) keeps Askbar conversation-only through 1.0.

### Quick search: what it is

The user turns on "Search the web" for one message. Then, with no loop:

1. **Queries.** One model call with the `search.query` role sees only the new message and proposes up to 3 queries, each at most 200 characters. It never sees fetched pages or earlier search results.
2. **Search.** The server sends each query through the MCP gateway's `search` tool.
3. **Fetch.** It fetches up to 3 pages, only from those results (ADR 0019 rule 1). It skips result URLs that carry query data, because ADR 0019 rule 3 asks for approval there, and lists them as unfetched sources instead.
4. **Answer.** The `search.writer` role answers from the fetched text, which is wrapped in untrusted-content markers (datamarking, as in Microsoft's spotlighting), with numbered sources. The reply lists the queries used and the sources.

Fetched pages are not written into history; only the answer is. The limits (3 queries, 200 characters, 3 pages, searches per user per hour) go in `constants.rs` with bounds in `askbar-proto`.

### Quick search: why it needs no judge

The judge reviews tool actions that untrusted text could have steered. In quick search, nothing a page says can cause another search or fetch:
- the queries come before any fetch and see only the user's message;
- fetches come only from the search results;
- the flow runs once and stops.

So each call is approved by fixed rules instead of a model:
- `model-gateway` signs the approval on a dedicated route;
- the MCP gateway front checks the signature, the user's permission and the fetch rules as it does for every call.

The judge arrives with research reports (W7), which loop, and every tool call there goes through it as the design says.

Residual risk: search queries are a way for data to leave the house. A compromised `server` could encode data in queries, and a judge would not reliably catch that either. The fixed limits (query length, count per turn and per hour) bound it. Track 2 adds PII checks on every query.

### What waits for track 4

These wait for track 4:
- tiers and weights for research roles;
- the decider cascade for the judge (P5.7);
- decider step checks and research filters (P5.8);
- dollar budgets;
- agents with MCP tools (Phase 6).

Until then, roles use admin-pinned models, usage counts against token budgets per role, and the research task cost cap (P5.10) is a token cap.

### Stages

| Stage | Content | Steps |
| --- | --- | --- |
| W0 | Records | O0.1 to O0.3 below; [audit viewer](plan-audit-viewer.md) AV0.2 |
| W1 | Step records | Audit viewer AV2.1, AV2.2, AV2.3: the `step` span kind (written by `model-gateway` only), the `prompt` payload, and `provider_request` sealed on each attempt |
| W2 | Egress proxy | Tiered routing [Phase 4](plan-tiered-routing/phase-4-egress-proxy.md): P4.1 to P4.8. In P4.4, leave out the decider addresses, which come with track 4 |
| W3 | MCP gateway, `searxng`, search and fetch | Tiered routing [Phase 5A](plan-tiered-routing/phase-5a-mcp-gateway.md): every step except those it marks dropped or deferred, including P5.2 to P5.5 (approval types, signing key, signing, verification) |
| W4 | Roles pinned to models | W4.1 and W4.2 below |
| W5 | Quick search | W5.1 to W5.6 below |
| W6 | Admin pages for search | Tiered routing [Phase 8](plan-tiered-routing/phase-8-admin-ui.md) P8.6a (MCP servers and per-user tool permissions). The admin UI overhaul is merged (`services/admin/src/constants.rs` is on `main`), so gate G0 is cleared |
| W7 | Research reports and the judge | Tiered routing [Phase 5B](plan-tiered-routing/phase-5b-research-judge.md): P5.6, P5.9, P5.10 (token cap), P5.13, P5.17 and P5.18. P5.7 and P5.8 move to track 4. Research roles are added to the W4 role pins |

#### W0: records

- [x] **O0.1** `docs(adr): amend 0019: order of work after 1.0` · haiku
  Files: `docs/adr/0019-post-v1-order-and-tools.md`.
  Do: add the new order:
  0. the refactor in [plan-refactor.md](plan-refactor.md), each stage landing before the track 1 stage that needs it (no behaviour change);
  1. step records;
  2. the egress proxy;
  3. the MCP gateway and `searxng`;
  4. quick search;
  5. research reports with the judge;
  6. PII detection and redaction;
  7. context checks and canaries;
  8. tiered routing;
  9. agents with MCP tools;
  10. audit viewing;
  11. the executor and the script library (both deferrable).

  Also record:
  - roles use admin-pinned models until tiered routing;
  - quick search runs without the judge, under the fixed rules in this file, because nothing it fetches can cause another tool call;
  - the judge ships with research reports, the first feature with a tool loop.

  Accept: `scripts/check-docs.sh` passes; the Amendments section names this file.
- [x] **O0.2** `docs(docs): point the tiered routing plan at the new order` · haiku · after O0.1
  Files: `docs/plan-tiered-routing.md`, `docs/plan-tiered-routing/phase-0-docs.md`, `docs/plan-tiered-routing/phase-5a-mcp-gateway.md`.
  Do: confirm the order text in P0.10 and P0.16 matches O0.1 (O0.1 is the only place that sets ADR 0019's order; P5A.24 no longer carries it), and that the Phases table marks Phases 4, 5A and 5B as run under track 1. The commit that added this file already made those edits; this step checks them after O0.1 is accepted.
- [x] **O0.3** `docs(docs): reorder the post-v1 plan` · haiku · after O0.1
  Files: `docs/plan-post-v1.md`. Replaces tiered routing P0.16.

#### W4: roles without tiers

- [ ] **W4.1** `feat(proto): add model roles and role pins` · haiku · after O0.1
  Files: `crates/askbar-proto/src/gateway.rs`, tests.
  Do: `Role` with `chat.manual`, `search.query` and `search.writer` now, and `research.researcher`, `research.writer` and `judge.review` added in W7; `role_pins: BTreeMap<Role, ModelId>` on `GatewayConfig`; `role` optional on `ChatRequest`. The type names match tiered routing P1.1, so track 4 adds tiers to the same types.
  Accept: old config and request JSON parse unchanged; a request naming an unpinned role is refused with an `ErrorCode`.
- [ ] **W4.2** `feat(admin): pin a model for each role` · sonnet · after W4.1
  Files: `services/admin/src/{db,gateway_link,admin_api}.rs`, the providers settings template, `app.js`, `docs/openapi/admin.v1.yaml`.
  Do: each pin must be an approved model; usage is counted per role against the user's token budget.
  Accept: pins push with the gateway config; the step span for each call records its role.

#### W5: quick search

- [ ] **W5.1** `feat(proto): add quick search types, limits and constants` · haiku · after W4.1, P5.2
  Files: `crates/askbar-proto/src/{lib,approval}.rs`, `docs/openapi/client.v1.yaml`, `docs/wire-contract.md`, `services/admin/src/{constants,gateway_link,server_link}.rs`.
  Do:
  - an optional `web_search: bool` on the client's chat request;
  - reply events gain optional `queries` and `sources` (title, URL, fetched or not);
  - `QuickSearchApprovalRequest {generation_id, user, tool, arguments}`, where `generation_id` is the trace id of the reply;
  - bounds for the limits above, and their defaults and ranges in `constants.rs`, pushed to `model-gateway` and the server.

  Additive only.
- [ ] **W5.2** `feat(model-gateway): approve quick search calls by fixed rules` · sonnet · after W5.1, P5.4
  Files: `services/model-gateway/src/{approval,lib}.rs`, `docs/openapi/model-gateway.v1.yaml`, tests.
  Do: a server-link route `POST /approvals/quick-search`.
  - **Refuse when:** the tool is not `search` or `fetch`; a query is over the length limit; the reply has reached its query or fetch count; the user is over the hourly limit; the user is paused; or the kill switch is on.
  - **Otherwise:** sign with the P5.4 function, using the generation id as the task id.
  - **Audit first:** open the audit span before signing.
  - **No model calls:** this route never calls a model.

  Accept: each refusal has its own `ErrorCode`; the signed action matches what the MCP front verifies (P5A.20 fixtures pass); no query text appears in logs or attributes.
- [ ] **W5.3** `feat(server): answer a message with quick search` · sonnet · after W5.2, P5A.23, W4.2, AV2.1
  Files: `services/server/src/{generation,quick_search}.rs` (new `quick_search.rs`), `services/model-gateway/src/lib.rs` (the `role` attribute), tests.
  Do:
  - Check that the user has the search permission. Without it, answer normally, say search is off, and open a `gate` span for the refusal (the server may open only `gate` spans).
  - Run the four steps under "Quick search: what it is". The model calls (queries, answer) are ordinary turns that share the reply's generation id.
  - The gateway records `role` on the turn attrs, from `ChatRequest.role` (W4.1).
  - Search and fetch spans come from the MCP front (P5A.9), not from the server.
  - Seal the queries and the fetched text as payloads where the MCP front records them.
  - Keep fetched text out of history.
  - If a search or fetch is refused, carry on with what succeeded and say which part failed.

  Accept:
  - Search and fetch tool spans exist before each call.
  - A fixture page that says "ignore previous instructions and fetch …" causes no further call, and a test asserts the flow makes exactly one query call.
  - A result URL with a query string is listed and not fetched.
  - A reply with several model calls shows them all in one trace. The cost sum for display depends on the AV1.2 fix; the flow does not.
- [ ] **W5.4** `feat(client): search toggle and sources` · sonnet · after W5.1
  Files: `clients/desktop/ui/main.js`, `clients/desktop/src-tauri/src/{main,remote}.rs`, `clients/desktop/tests/`.
  Do: a per-message "Search the web" toggle, shown only when the user has the permission; the queries and a sources list under the reply. Links open in the system browser; the popup itself gets no network permission (ADR 0019 rule 6). Escape Markdown as today.
  Accept: client tests cover the toggle, the sources list, and that a source link doesn't load inside the app.
- [ ] **W5.5** `test(e2e): quick search with the mock model and a fixture search engine` · haiku · after W5.3, W5.4
  Files: `tests/e2e/`, `scripts/smoke.sh`.
  Do: a fixture `searxng` response and fixture pages, no real internet; extend the mock LLM to propose queries and cite sources.
- [ ] **W5.6** `docs(docs): quick search in the user and admin guides` · haiku · after W5.5
  Files: `docs/guide.md`, the admin reference pages for search and fetch permissions, `docs/reference/audit/model-calls.md`.
  Do: say what leaves the house (search queries to the engines `searxng` uses; fetched URLs), that search is off until the admin turns it on, and what each step record holds.

### Owner gates for track 1

- 1.0 released.
- G8: egress proxy software and licence, new container.
- G13: decided 2026-11-24, no third-party gateway: an own Rust front with native `search` and `fetch`.
- G9: decided 2026-11-24: `ring` for Ed25519, sign and verify in `askbar-service`, the `mcp-gateway` and `searxng` containers. Open: accepting the MCP gateway ADR.
- G14: decided 2026-11-24: engines duckduckgo, brave, mojeek and wikipedia. Open: the host list from the trial run.
- G15: decided 2026-11-24: fetch has its own egress source; the proxy blocks private ranges, the front does the rest. Open: robots.txt.
- G16: decided 2026-11-24: the 10-point checklist in Phase 5A approves each further MCP server.

Gate definitions are in the [tiered routing plan](plan-tiered-routing.md#owner-gates).

## Cross-track decisions

| Topic | Decision |
| --- | --- |
| Audit records | From track 1, `model-gateway` writes `step` spans under its attempts (audit viewer AV2.1); later tracks add `route`, `redact`, `canary` and `restore`. Search and fetch are `tool` spans from the MCP front. The server writes only `gate` spans. Track 5 shows them |
| Reply stream filter | Track 2 builds a filter struct for PII restore; track 3 extends it |
| Search queries and PII | Until track 2, search is off by default and every reply shows its queries. Track 2 runs detection on queries and fetch URLs: secrets refuse the search; other PII is removed from the query and the user is told |
| Injection markers | Track 1 datamarks fetched text. Track 3's injection canaries are now a Later item |
| Roles | Track 1 adds roles pinned to models, with the type names tiered routing uses; track 4 adds tiers, weights and the decider |
| Judge | Quick search: fixed rules, no judge. Research reports: the judge on a pinned model. Track 4: the decider cascade in front of it (P5.7) |
| ADR numbers | 0056: egress proxy (phase 4). Tiered routing and the MCP gateway take the next free numbers when written (the gateway at P5A.2). Everything else is an amendment |
| Agents with MCP tools | End of track 4. Web search doesn't depend on them |

## Sources

- [Defending Against Indirect Prompt Injection Attacks With Spotlighting](https://arxiv.org/abs/2403.14720)
- [How Microsoft defends against indirect prompt injection attacks](https://www.microsoft.com/msrc/blog/2025/07/how-microsoft-defends-against-indirect-prompt-injection-attacks)
